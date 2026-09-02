//! Admin-managed query deny list: a small set of admin-authored rules, stored in Postgres, cached
//! in every `flight-sql-srv` replica, and evaluated at the front of `execute_query` -- before the
//! session context is built and before planning -- so a matching query is rejected for a few
//! microseconds of work instead of a memory-pool reservation and a wave of object-store reads.
//!
//! A rule is a boolean SQL expression over a fixed, documented match context ([`match_schema`])
//! -- parsed and evaluated by DataFusion itself, so there is no grammar of our own to specify or
//! keep in sync, and no evaluator of our own to get subtly wrong. Rules stand until an admin
//! removes them explicitly; there is no expiry.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use datafusion::arrow::array::{Array, ArrayRef, BooleanArray, RecordBatch, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::common::{DFSchema, plan_err};
use datafusion::execution::context::SessionContext;
use datafusion::logical_expr::{ColumnarValue, ExprSchemable};
use datafusion::physical_plan::PhysicalExpr;
use datafusion::sql::sqlparser::dialect::GenericDialect;
use datafusion::sql::sqlparser::tokenizer::{Token, Tokenizer};
use micromegas_tracing::intern_string::intern_string;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set::{Property, PropertySet};
use sqlx::Row;
use std::future::Future;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::RwLock;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use uuid::Uuid;

/// `{prefix}`-less env var: snapshot refresh / `last_hit_at` flush interval, and the bound on
/// cross-replica propagation of a newly inserted or removed rule.
pub const MICROMEGAS_QUERY_DENY_REFRESH_SECONDS: &str = "MICROMEGAS_QUERY_DENY_REFRESH_SECONDS";
/// Default value of [`MICROMEGAS_QUERY_DENY_REFRESH_SECONDS`].
pub const DEFAULT_REFRESH_SECONDS: u64 = 10;
/// Env var: cap on the number of rules in force at once (bounds per-query evaluation cost).
pub const MICROMEGAS_QUERY_DENY_MAX_RULES: &str = "MICROMEGAS_QUERY_DENY_MAX_RULES";
/// Default value of [`MICROMEGAS_QUERY_DENY_MAX_RULES`].
pub const DEFAULT_MAX_RULES: usize = 100;

fn refresh_interval() -> Duration {
    let secs = std::env::var(MICROMEGAS_QUERY_DENY_REFRESH_SECONDS)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_REFRESH_SECONDS)
        .max(1);
    Duration::from_secs(secs)
}

/// The cap on rules in force at once, from [`MICROMEGAS_QUERY_DENY_MAX_RULES`] (default
/// [`DEFAULT_MAX_RULES`]).
pub fn max_rules() -> usize {
    std::env::var(MICROMEGAS_QUERY_DENY_MAX_RULES)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_RULES)
}

// ---------------------------------------------------------------------------------------------
// Normalized SQL fingerprint
// ---------------------------------------------------------------------------------------------

/// Literal-stripped fingerprint of a statement: the first 16 hex chars of the SHA-256 of the
/// normalized token stream. Tokenizes internally; the token stream is an implementation detail
/// and does not appear in the signature.
///
/// This is what makes the dashboard case work: consecutive refreshes of the same panel differ
/// only in their time-range literals, so they collapse to one fingerprint. Tokenization failure
/// (unparseable SQL) falls back to hashing the whitespace-collapsed raw text, so a fingerprint
/// always exists.
pub fn fingerprint_of(sql: &str) -> String {
    let normalized = match Tokenizer::new(&GenericDialect {}, sql).tokenize() {
        Ok(tokens) => normalize_tokens(&tokens),
        Err(_) => collapse_whitespace(sql),
    };
    hash_hex16(&normalized)
}

/// Joins the normalized token text with a single space: every `Token::Number`/string-literal
/// variant becomes `?`; whitespace and comments (both represented as `Token::Whitespace`) are
/// dropped; a `Token::Word` is lowercased only when it carries no `quote_style` -- a quoted word
/// (e.g. `"processes.exe"`) is kept verbatim, case included, via its own `Display` impl, which is
/// what keeps two differently-cased quoted identifiers from colliding into the same fingerprint.
/// Everything else uses the token's own `Display` impl unchanged.
fn normalize_tokens(tokens: &[Token]) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(tokens.len());
    for tok in tokens {
        match tok {
            Token::Whitespace(_) => continue,
            Token::Number(_, _)
            | Token::SingleQuotedString(_)
            | Token::NationalStringLiteral(_)
            | Token::HexStringLiteral(_)
            | Token::EscapedStringLiteral(_)
            | Token::TripleSingleQuotedString(_)
            | Token::TripleDoubleQuotedString(_) => parts.push("?".to_string()),
            Token::Word(w) if w.quote_style.is_none() => parts.push(w.value.to_lowercase()),
            other => parts.push(other.to_string()),
        }
    }
    parts.join(" ")
}

fn collapse_whitespace(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn hash_hex16(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(s.as_bytes());
    let mut hex = hex::encode(digest);
    hex.truncate(16);
    hex
}

// ---------------------------------------------------------------------------------------------
// The match context and expression compilation
// ---------------------------------------------------------------------------------------------

/// The fixed, documented schema every deny rule is a predicate over -- every attribute
/// `execute_query` has already resolved by the time the check runs, in the same column order as
/// [`QueryAttribution::to_batch`]. Every field is nullable `Utf8`: NULL semantics come from SQL
/// and are the ones we want (e.g. `notebook = 'fleet-overview'` evaluates to NULL, not true, for
/// a query that carried no notebook header, so the rule does not fire).
///
/// The identity column is named `user_id`, not `user`: DataFusion's default SQL dialect parses a
/// bare `user` in expression position as the zero-argument function call `user()`, not a column
/// reference, and no such scalar function is registered here -- `user = 'jean'` would fail at
/// planning with "Invalid function 'user'".
///
/// Built once, in [`MATCH_SCHEMA`]: constructing a `DFSchema` runs `check_names()` (a `BTreeSet`
/// insert per field) on top of allocating the `Fields`/`Schema` themselves, and this is called on
/// every query while any deny rule stands -- so `match_schema()` and [`QueryAttribution::to_batch`]
/// both clone the cached `DFSchema`/`Arc<Schema>` rather than rebuilding it.
static MATCH_SCHEMA: LazyLock<DFSchema> = LazyLock::new(|| {
    let arrow_schema = Schema::new(vec![
        Field::new("user_id", DataType::Utf8, true),
        Field::new("email", DataType::Utf8, true),
        Field::new("service_account", DataType::Utf8, true),
        Field::new("client", DataType::Utf8, true),
        Field::new("agent", DataType::Utf8, true),
        Field::new("entrypoint", DataType::Utf8, true),
        Field::new("session", DataType::Utf8, true),
        Field::new("notebook", DataType::Utf8, true),
        Field::new("cell", DataType::Utf8, true),
        Field::new("client_ip", DataType::Utf8, true),
        Field::new("sql", DataType::Utf8, true),
        Field::new("sql_hash", DataType::Utf8, true),
    ]);
    DFSchema::try_from(arrow_schema).expect("match_schema is a fixed, valid schema")
});

pub fn match_schema() -> DFSchema {
    MATCH_SCHEMA.clone()
}

/// Borrowed view of what `execute_query` has already resolved, in the same column order as
/// [`match_schema`]. `service_account`/`session`/`notebook`/`cell` are `Option` because the
/// corresponding header may be absent; every other field always has a value (falling back to
/// `"unknown"` upstream when nothing better is available).
pub struct QueryAttribution<'a> {
    pub user_id: &'a str,
    pub email: &'a str,
    pub service_account: Option<&'a str>,
    pub client: &'a str,
    pub agent: &'a str,
    pub entrypoint: &'a str,
    pub session: Option<&'a str>,
    pub notebook: Option<&'a str>,
    pub cell: Option<&'a str>,
    pub client_ip: &'a str,
    pub sql: &'a str,
    pub sql_hash: &'a str,
}

impl QueryAttribution<'static> {
    /// A fixed, fully-populated attribution used only to canary-evaluate a compiled expression
    /// once at compile time (see [`compile_match_expr`]'s step 4) -- its values are never
    /// meaningful, only whether comparing against them errors.
    fn probe() -> Self {
        Self {
            user_id: "probe",
            email: "probe",
            service_account: Some("probe"),
            client: "probe",
            agent: "probe",
            entrypoint: "probe",
            session: Some("probe"),
            notebook: Some("probe"),
            cell: Some("probe"),
            client_ip: "probe",
            sql: "probe",
            sql_hash: "probe",
        }
    }
}

impl QueryAttribution<'_> {
    /// One-row `RecordBatch` over [`match_schema`], in column order -- the only thing
    /// [`QueryDenyList::check`] builds per query, and the only allocation on the deny-check path.
    pub fn to_batch(&self) -> RecordBatch {
        let schema = MATCH_SCHEMA.inner().clone();
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(vec![Some(self.user_id)])),
            Arc::new(StringArray::from(vec![Some(self.email)])),
            Arc::new(StringArray::from(vec![self.service_account])),
            Arc::new(StringArray::from(vec![Some(self.client)])),
            Arc::new(StringArray::from(vec![Some(self.agent)])),
            Arc::new(StringArray::from(vec![Some(self.entrypoint)])),
            Arc::new(StringArray::from(vec![self.session])),
            Arc::new(StringArray::from(vec![self.notebook])),
            Arc::new(StringArray::from(vec![self.cell])),
            Arc::new(StringArray::from(vec![Some(self.client_ip)])),
            Arc::new(StringArray::from(vec![Some(self.sql)])),
            Arc::new(StringArray::from(vec![Some(self.sql_hash)])),
        ];
        RecordBatch::try_new(schema, arrays)
            .expect("match_schema and QueryAttribution::to_batch are kept in sync")
    }
}

/// Parses, validates, and plans `match_expr` into an `Arc<dyn PhysicalExpr>` -- the whole
/// evaluator, correct by construction because DataFusion does the parsing and the evaluation.
///
/// 1. **Parse** with `ctx.parse_sql_expr`, against [`match_schema`]. `ctx` is expected to be a
///    bare `SessionContext::new()` (no lakehouse tables or catalog registered) -- see
///    [`QueryDenyList`]'s doc comment for why the same bare context is used both at refresh and
///    from `deny_queries`.
/// 2. **Validate** with exactly two checks: the result type is `Boolean`, and at least one column
///    is referenced. An unknown column or function already fails at step 1; a subquery,
///    aggregate, or window function cannot be lowered to a scalar `PhysicalExpr` and fails at
///    step 3 -- each with DataFusion's own diagnostic.
/// 3. **Plan** the validated `Expr` into a `PhysicalExpr` with the free-function
///    `datafusion::physical_expr::create_physical_expr` -- deliberately *not*
///    `SessionContext::create_physical_expr`/`SessionState::create_physical_expr`, both of which
///    run the expression through `ExprSimplifier::coerce` first and would silently insert a
///    `CAST` for a type-mismatched comparison (`client = 42` becomes `CAST(client AS Int64) =
///    42`, which compiles, then either errors on every real value or -- worse -- happens to
///    parse and quietly matches something the admin never intended). The free function's own
///    doc comment is explicit that it performs no coercion ("There should be no coercion during
///    physical planning") -- but it also performs no operand-type *checking* at this stage:
///    `binary()` happily builds a `BinaryExpr` node comparing a `Utf8` column against an
///    `Int64`/`Timestamp` literal, and Arrow's comparison kernels are the ones that actually
///    refuse it, only once the node is evaluated. Left there, that would be exactly the "silent
///    per-query failure" this design wants to avoid -- so step 4 forces that evaluation now.
/// 4. **Canary-evaluate** the compiled expression once against [`QueryAttribution::probe`], a
///    fixed one-row batch that exists for no other reason. Every match-context column is
///    `Utf8`, so the only way this can fail is the type mismatch step 3 doesn't catch; catching
///    it here turns "never fires" into a loud compile-time error, at the one-time cost of a
///    single evaluate() call per rule (refresh/`deny_queries` time, never per query).
pub fn compile_match_expr(
    ctx: &SessionContext,
    match_expr: &str,
) -> datafusion::error::Result<Arc<dyn PhysicalExpr>> {
    let schema = match_schema();
    let expr = ctx.parse_sql_expr(match_expr, &schema)?;
    let ty = expr.get_type(&schema)?;
    if ty != DataType::Boolean {
        return plan_err!("query deny rule must be a boolean expression, got {ty}: {match_expr}");
    }
    if expr.column_refs().is_empty() {
        return plan_err!(
            "query deny rule must reference at least one match-context column -- a rule with \
             no column reference would deny every query in the deployment: {match_expr}"
        );
    }
    let physical = datafusion::physical_expr::create_physical_expr(
        &expr,
        &schema,
        &datafusion::execution::context::ExecutionProps::new(),
    )?;
    let probe_batch = QueryAttribution::probe().to_batch();
    physical.evaluate(&probe_batch).map_err(|e| {
        datafusion::error::DataFusionError::Plan(format!(
            "query deny rule failed a compile-time check, likely comparing a column against a \
             mismatched type: {e}: {match_expr}"
        ))
    })?;
    Ok(physical)
}

/// The primary escape hatch: the deny-list check is skipped when the caller can reach
/// the admin functions at all -- the same predicate `register_lakehouse_functions` gates
/// `deny_queries`/`remove_query_denial` on -- **and** the statement mentions one of
/// `deny_queries` / `remove_query_denial` / `list_query_denials`: three `contains` checks over
/// the lowercased SQL text, nothing more. No call-position or token analysis: this only ever
/// runs for a query that is *already* being denied, and the gate it sits behind means any caller
/// it opens for could call those three functions directly anyway.
pub fn skip_for_admin_recovery(sql: &str, is_admin: bool) -> bool {
    if !is_admin {
        return false;
    }
    let lowered = sql.to_lowercase();
    lowered.contains("deny_queries")
        || lowered.contains("remove_query_denial")
        || lowered.contains("list_query_denials")
}

// ---------------------------------------------------------------------------------------------
// Rule model, evaluation, and cache
// ---------------------------------------------------------------------------------------------

/// The DB row, exactly as `list_query_denials()` returns it and as `insert` echoes back. No
/// compiled expression or in-process timestamp here -- those exist only for a rule that has been
/// compiled into a [`DenySnapshot`], and a freshly inserted or listed row has not necessarily
/// been through that yet on every replica.
#[derive(Debug, Clone)]
pub struct QueryDenyRow {
    pub rule_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub reason: String,
    pub match_expr: String,
    /// `None` until the rule first fires; accurate to within one refresh tick.
    pub last_hit_at: Option<DateTime<Utc>>,
}

/// A row compiled into one snapshot: the row, its planned expression, and the one piece of
/// in-process state a rule accumulates. Lives only inside a [`DenySnapshot`].
#[derive(Debug)]
pub struct QueryDenyRule {
    pub row: QueryDenyRow,
    expr: Arc<dyn PhysicalExpr>,
    /// Unix seconds of the most recent hit since the last flush; `0` means "not hit since the
    /// last flush". Flushed (and reset to `0`) by [`QueryDenyList::refresh`].
    last_hit: AtomicI64,
}

impl QueryDenyRule {
    /// `pub` so tests can build a rule directly from a [`compile_match_expr`] result, without
    /// going through Postgres -- see [`QueryDenyList::with_snapshot`].
    pub fn new(row: QueryDenyRow, expr: Arc<dyn PhysicalExpr>) -> Self {
        Self {
            row,
            expr,
            last_hit: AtomicI64::new(0),
        }
    }

    /// Records that this rule matched a query, for the next [`QueryDenyList::refresh`] tick to
    /// flush into `last_hit_at`. Racing hits within one tick are fine -- `fetch_max` keeps the
    /// most recent, and that's all "is this rule still firing?" needs.
    pub fn record_hit(&self) {
        let now = Utc::now().timestamp();
        self.last_hit.fetch_max(now, Ordering::Relaxed);
    }
}

/// Tags a `query_denied`/`query_deny_compile_error_count` metric with the rule id it concerns.
/// Interned via [`intern_string`] (same pattern as `maintenance.rs`'s per-view-set metric tags):
/// bounded cardinality (a rule id exists only while it's part of a deployment's rule set, capped
/// at [`max_rules`]), so leaking one interned string per rule id that has ever existed is cheap.
pub fn rule_tags(rule_id: &Uuid) -> &'static PropertySet {
    PropertySet::find_or_create(vec![Property::new(
        "rule_id",
        intern_string(&rule_id.to_string()),
    )])
}

/// The compiled rules, ordered by `(created_at, rule_id)`, oldest first -- so the first match is
/// always the oldest matching rule, on every replica. An alias, not a newtype: nothing hangs
/// behind it. `Arc<[_]>` rather than `Arc<Vec<_>>`:
/// the snapshot is immutable once built, and this drops a pointer hop off the per-query path.
pub type DenySnapshot = Arc<[Arc<QueryDenyRule>]>;

fn empty_snapshot() -> DenySnapshot {
    Arc::from(Vec::new())
}

/// Sorts `rules` by `(created_at, rule_id)`, oldest first, and wraps them into a [`DenySnapshot`]
/// -- `pub` so tests can build one directly, matching the invariant [`QueryDenyList::refresh`]
/// and [`QueryDenyList::insert`] maintain internally.
pub fn sorted_snapshot(mut rules: Vec<Arc<QueryDenyRule>>) -> DenySnapshot {
    rules.sort_by_key(|r| (r.row.created_at, r.row.rule_id));
    Arc::from(rules)
}

/// Owns the Postgres-backed rule store and the compiled, in-memory snapshot every query is
/// checked against. One instance is owned by `LakehouseContext`, mirroring `AudienceIndex`.
///
/// `ctx` is a bare `SessionContext::new()` -- no lakehouse tables or catalog registered against
/// it, just DataFusion's parser and name resolution -- held only so [`compile_match_expr`] can be
/// called against it. Both `refresh` and `deny_queries`'s `call_with_args` compile through this
/// same context, so rule compilation produces the same result regardless of whose session
/// triggered it.
pub struct QueryDenyList {
    pool: sqlx::Pool<sqlx::Postgres>,
    ctx: SessionContext,
    snapshot: RwLock<DenySnapshot>,
    /// Serializes `insert`/`delete`/`refresh`'s DB-operation-plus-snapshot-swap against each
    /// other, held across the whole sequence (including the `await`s) rather than just the
    /// final snapshot edit. Without this, `refresh` reading the table in `load_rows` and a
    /// concurrent `delete` performing its own DELETE-plus-snapshot-edit could interleave so that
    /// `refresh`'s stale, pre-delete row set overwrites the snapshot `delete` just fixed up,
    /// silently re-enforcing a rule the admin was just told was removed (or the mirror case,
    /// losing a freshly inserted rule) until the next tick. Serializing the three methods means
    /// a `delete`/`insert` either finishes entirely before a `refresh` starts reading the table,
    /// or starts entirely after `refresh`'s swap -- either way it sees, and leaves, a consistent
    /// snapshot. `flush_hits` is not covered: it only updates each rule's `last_hit` in place and
    /// never replaces the snapshot, so it cannot lose an insert/delete.
    write_lock: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for QueryDenyList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryDenyList")
            .field("rules", &self.snapshot.read().expect("lock").len())
            .finish()
    }
}

impl QueryDenyList {
    pub fn new(pool: sqlx::Pool<sqlx::Postgres>) -> Self {
        Self {
            pool,
            ctx: SessionContext::new(),
            snapshot: RwLock::new(empty_snapshot()),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Builds a `QueryDenyList` around an already-compiled `snapshot`, bypassing Postgres
    /// entirely -- for tests exercising [`Self::check`]'s in-memory semantics without a live DB
    /// (`pool` is unused by `check`; a `connect_lazy` pool that never touches the network is
    /// enough). Production code should use [`Self::new`] plus [`Self::refresh`].
    pub fn with_snapshot(pool: sqlx::Pool<sqlx::Postgres>, snapshot: DenySnapshot) -> Self {
        Self {
            pool,
            ctx: SessionContext::new(),
            snapshot: RwLock::new(snapshot),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// A bare `SessionContext` with no lakehouse tables or catalog registered, for compiling a
    /// `match_expr` outside this `QueryDenyList` (e.g. from `deny_queries`'s `call_with_args`,
    /// which is handed the caller's own session and needs a SQL-expression parser instead).
    pub fn compile_ctx(&self) -> &SessionContext {
        &self.ctx
    }

    /// The number of rules currently compiled into the snapshot -- what `deny_queries` checks
    /// against [`max_rules`] before inserting one more.
    pub fn rule_count(&self) -> usize {
        self.snapshot.read().expect("lock").len()
    }

    /// Checks `q` against every compiled rule, in order, returning the first (oldest) match.
    /// Returns immediately on an empty snapshot -- the steady state of a deployment that is not
    /// mid-incident -- before building the one-row batch every non-empty check needs.
    pub fn check(&self, q: &QueryAttribution<'_>) -> Option<Arc<QueryDenyRule>> {
        let snapshot = self.snapshot.read().expect("lock").clone();
        if snapshot.is_empty() {
            return None;
        }
        let batch = q.to_batch();
        for rule in snapshot.iter() {
            match rule.expr.evaluate(&batch) {
                Ok(ColumnarValue::Array(array)) => {
                    if let Some(bools) = array.as_any().downcast_ref::<BooleanArray>()
                        && bools.len() == 1
                        && !bools.is_null(0)
                        && bools.value(0)
                    {
                        return Some(rule.clone());
                    }
                }
                Ok(ColumnarValue::Scalar(datafusion::scalar::ScalarValue::Boolean(Some(true)))) => {
                    return Some(rule.clone());
                }
                Ok(ColumnarValue::Scalar(_)) => {}
                Err(e) => {
                    warn!(
                        "query_deny_list: rule {} failed to evaluate, skipping: {e:#}",
                        rule.row.rule_id
                    );
                }
            }
        }
        None
    }

    async fn load_rows(&self) -> Result<Vec<QueryDenyRow>> {
        let records = sqlx::query(
            "SELECT rule_id, created_at, created_by, reason, match_expr, last_hit_at \
             FROM query_deny_list \
             ORDER BY created_at, rule_id",
        )
        .fetch_all(&self.pool)
        .await
        .context("loading query_deny_list rows")?;
        records
            .into_iter()
            .map(|row| {
                Ok(QueryDenyRow {
                    rule_id: row.try_get("rule_id")?,
                    created_at: row.try_get("created_at")?,
                    created_by: row.try_get("created_by")?,
                    reason: row.try_get("reason")?,
                    match_expr: row.try_get("match_expr")?,
                    last_hit_at: row.try_get("last_hit_at")?,
                })
            })
            .collect()
    }

    /// Every rule currently in force, read fresh from Postgres (not the in-memory snapshot) --
    /// what `list_query_denials()` returns.
    pub async fn list(&self) -> Result<Vec<QueryDenyRow>> {
        self.load_rows().await
    }

    /// Inserts a new rule: `compiled` must already be the result of a successful
    /// [`compile_match_expr`] call against `match_expr` (typically the same call
    /// `deny_queries`'s `call_with_args` just made to validate it) -- `insert` stores
    /// `match_expr` verbatim and does not re-validate it. Refreshes the local snapshot
    /// synchronously before returning, so the admin who created the rule sees it in their own
    /// `list_query_denials()` immediately; other replicas pick it up within one refresh tick.
    pub async fn insert(
        &self,
        match_expr: &str,
        compiled: Arc<dyn PhysicalExpr>,
        reason: &str,
        created_by: &str,
    ) -> Result<QueryDenyRow> {
        let row = QueryDenyRow {
            rule_id: Uuid::new_v4(),
            created_at: Utc::now(),
            created_by: created_by.to_string(),
            reason: reason.to_string(),
            match_expr: match_expr.to_string(),
            last_hit_at: None,
        };
        // Held across the INSERT and the snapshot edit -- see `write_lock`'s doc comment -- so a
        // concurrent `refresh` cannot read the table before this INSERT and then overwrite the
        // snapshot edit below with that stale, pre-insert row set.
        let _guard = self.write_lock.lock().await;
        sqlx::query(
            "INSERT INTO query_deny_list (rule_id, created_at, created_by, reason, match_expr) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(row.rule_id)
        .bind(row.created_at)
        .bind(&row.created_by)
        .bind(&row.reason)
        .bind(&row.match_expr)
        .execute(&self.pool)
        .await
        .context("inserting query_deny_list row")?;
        let rule = Arc::new(QueryDenyRule::new(row.clone(), compiled));
        {
            let mut snapshot = self.snapshot.write().expect("lock");
            let mut rules: Vec<Arc<QueryDenyRule>> = snapshot.iter().cloned().collect();
            rules.push(rule);
            *snapshot = sorted_snapshot(rules);
        }
        Ok(row)
    }

    /// Deletes a rule by id, refreshing the local snapshot synchronously. Returns `true` if a row
    /// was actually deleted -- `false` lets the caller (`remove_query_denial`) return a clear
    /// "no such rule" message rather than a silent no-op.
    pub async fn delete(&self, rule_id: Uuid) -> Result<bool> {
        // Held across the DELETE and the snapshot edit -- see `write_lock`'s doc comment -- so a
        // concurrent `refresh` cannot read the table before this DELETE and then overwrite the
        // snapshot edit below with that stale, pre-delete row set.
        let _guard = self.write_lock.lock().await;
        let result = sqlx::query("DELETE FROM query_deny_list WHERE rule_id = $1")
            .bind(rule_id)
            .execute(&self.pool)
            .await
            .context("deleting query_deny_list row")?;
        let deleted = result.rows_affected() > 0;
        if deleted {
            let mut snapshot = self.snapshot.write().expect("lock");
            let rules: Vec<Arc<QueryDenyRule>> = snapshot
                .iter()
                .filter(|rule| rule.row.rule_id != rule_id)
                .cloned()
                .collect();
            *snapshot = Arc::from(rules);
        }
        Ok(deleted)
    }

    /// Flushes each rule's accumulated `last_hit`, then reloads the table and recompiles every
    /// `match_expr`. **A rule that fails to compile is skipped, never fatal** -- dropped from the
    /// snapshot with a `warn!` and a `query_deny_compile_error_count` metric, so an older replica
    /// that cannot compile a newer rule declines to enforce it rather than denying everything or
    /// crashing. **Fail-open on a failed reload**: the previous snapshot is kept, with a `warn!`
    /// and a `query_deny_refresh_error_count` metric -- the deny list is an availability valve,
    /// not a security control, and failing closed would deny every query on a Postgres blip.
    pub async fn refresh(&self) -> Result<()> {
        self.flush_hits().await;
        // Held from before `load_rows` through the final snapshot swap below -- see
        // `write_lock`'s doc comment -- so a concurrent `insert`/`delete` cannot land its own
        // DB-op-plus-snapshot-edit inside that window and have it clobbered by this stale read.
        let _guard = self.write_lock.lock().await;
        let rows = match self.load_rows().await {
            Ok(rows) => rows,
            Err(e) => {
                imetric!("query_deny_refresh_error_count", "count", 1_u64);
                warn!("query_deny_list: refresh failed, keeping previous snapshot: {e:#}");
                return Err(e);
            }
        };
        let mut compiled = Vec::with_capacity(rows.len());
        for row in rows {
            let rule_id = row.rule_id;
            match compile_match_expr(&self.ctx, &row.match_expr) {
                Ok(expr) => compiled.push(Arc::new(QueryDenyRule::new(row, expr))),
                Err(e) => {
                    imetric!(
                        "query_deny_compile_error_count",
                        "count",
                        rule_tags(&rule_id),
                        1_u64
                    );
                    warn!(
                        "query_deny_list: rule {rule_id} failed to compile, skipping (not \
                         enforced): {e:#}"
                    );
                }
            }
        }
        {
            // Carry each new rule's `last_hit` forward from its predecessor in the outgoing
            // snapshot, so a `record_hit()` landing on the old `Arc` between `flush_hits` and this
            // swap (two DB round-trips plus the compile loop above) isn't discarded along with it.
            // Held across the read-merge-write so the window in which such a hit could still be
            // lost is just the in-memory work below, not the whole refresh.
            let mut snapshot = self.snapshot.write().expect("lock");
            for rule in &compiled {
                if let Some(previous) = snapshot.iter().find(|p| p.row.rule_id == rule.row.rule_id)
                {
                    let carried = previous.last_hit.load(Ordering::Relaxed);
                    rule.last_hit.fetch_max(carried, Ordering::Relaxed);
                }
            }
            *snapshot = sorted_snapshot(compiled);
        }
        Ok(())
    }

    async fn flush_hits(&self) {
        let snapshot = self.snapshot.read().expect("lock").clone();
        for rule in snapshot.iter() {
            let last_hit = rule.last_hit.swap(0, Ordering::Relaxed);
            if last_hit == 0 {
                continue;
            }
            let hit_at = DateTime::<Utc>::from_timestamp(last_hit, 0).unwrap_or_else(Utc::now);
            if let Err(e) = sqlx::query(
                "UPDATE query_deny_list \
                 SET last_hit_at = greatest(coalesce(last_hit_at, $1), $1) \
                 WHERE rule_id = $2",
            )
            .bind(hit_at)
            .bind(rule.row.rule_id)
            .execute(&self.pool)
            .await
            {
                // Re-arm rather than leaving the atomic at the `swap(0)` above: a failed UPDATE
                // must not permanently lose the hit, since it's the next tick's only chance to
                // retry the write. `fetch_max` so a hit recorded concurrently (after our swap,
                // above) isn't clobbered by this re-arm.
                rule.last_hit.fetch_max(last_hit, Ordering::Relaxed);
                warn!(
                    "query_deny_list: failed to flush last_hit for rule {}: {e:#}",
                    rule.row.rule_id
                );
            }
        }
    }

    /// Spawns the background task that calls [`Self::refresh`] once immediately and then on
    /// every [`MICROMEGAS_QUERY_DENY_REFRESH_SECONDS`] tick, until `shutdown` resolves. Only the
    /// FlightSQL server builder spawns this; other `LakehouseContext` holders (maintenance
    /// daemon, tests) keep an empty snapshot and never deny anything.
    pub fn spawn_refresh_task(
        self: Arc<Self>,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) {
        tokio::spawn(async move {
            let mut shutdown = Box::pin(shutdown);
            loop {
                // Ignore the `Err`: `refresh()` has already warn!-logged and metered a failed
                // reload before returning it.
                let _ = self.refresh().await;
                tokio::select! {
                    _ = &mut shutdown => break,
                    _ = tokio::time::sleep(refresh_interval()) => {}
                }
            }
        });
    }
}
