//! Query Enforcement Prong B (#1371, AbAC Stage 3; extended by #1486) -- arg-addressed guards for
//! five entry points: the span/metadata UDTFs and the `get_payload` UDF that
//! [`super::ownership_rewrite::OwnershipRewrite`] (Prong A) structurally cannot reach (they bake
//! their target id into a provider at plan time, return schemas with no `process_id` column to
//! filter on, and some build their own inner session under `ReadScope::All`), plus
//! `view_instance(...)`, which Prong A *can* and does reach -- it row-filters every
//! `view_instance` scan the same as the named-table form -- but which still needs a scan-time
//! guard of its own to stop a caller from triggering JIT materialization (real compute and
//! object-storage writes) for an instance outside their audiences before Prong A's filter ever
//! gets to run. See `tasks/1371_udtf_udf_guards_plan.md` and
//! `tasks/1486_view_instance_guard_plan.md` for the full design rationale; this comment records
//! only what a future reader of this file needs close at hand.
//!
//! ## One cache, one question
//!
//! [`AudienceIndex`] answers exactly one question, for three id kinds ([`IdKind`]): which
//! audience is stamped on the process that owns this id? It resolves from **Postgres** (the
//! origin of `micromegas.audience`), by primary-key point query -- fresher and independent of
//! materialization, unlike [`super::ownership_rewrite::OwnershipRewrite`], which reads a
//! daemon-materialized snapshot (see the plan's §11 for the consequences of the two prongs
//! reading different copies).
//!
//! ## Fail-closed
//!
//! [`is_readable`] is the whole authorization rule, pure and offline-testable: `ReadScope::All`
//! passes everything; `ReadScope::Audiences` denies [`OwnerAudience::Unknown`] unconditionally
//! and matches [`OwnerAudience::Audience`] byte-exactly. There is no unstamped state any more
//! (#1482): a process whose credential carried no audience keeps no property in Postgres, and
//! `owner_query_sql`'s `COALESCE` resolves that to `MICROMEGAS_DEFAULT_AUDIENCE` here, so every
//! *existing* id has a real owning audience. `Unknown` therefore means "no such row" -- deny, on
//! the same fail-closed reasoning as before. An id ambiguous between a `process_id` and a `stream_id`
//! interpretation ([`OwnerAudience::Ambiguous`]) is readable only when every interpretation is --
//! never by picking one arm over the other. A resolution *error* (Postgres unreachable) is a
//! denial too -- [`AudienceGuard::authorize`]/[`AudienceGuard::readable_ids`] map it to a query
//! failure, never to a readable verdict.
//!
//! ## No existence oracle
//!
//! Every guard denial and every "no such id" produce the same error text (e.g. `process_spans:
//! 'xxx' not found or not accessible`) -- a distinct "permission denied" would let a caller
//! enumerate which ids exist in other audiences. The server log ([`debug!`]) records the real
//! reason so an operator can tell the two apart; the client cannot.

use super::read_scope::{CallerContext, ReadScope};
use crate::audience::AUDIENCE_PROPERTY;
use anyhow::Context;
use datafusion::common::plan_err;
use datafusion::error::DataFusionError;
use micromegas_tracing::prelude::*;
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// What a resolution attempt found. Every *existing* process resolves to an audience -- its own
/// if it was stamped, `MICROMEGAS_DEFAULT_AUDIENCE` if it was not (#1482) -- so `Unknown` is the
/// only "not a real, readable audience" state left, and it means "no such row" (not yet
/// ingested, or retention already deleted it, plan §11). Deny, fail-closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerAudience {
    Unknown,
    Audience(Arc<str>),
    /// The id resolved to more than one distinct owner under `IdKind::ProcessOrStream`'s two
    /// arms -- a `process_id`/`stream_id` collision (see the variant's doc comment on
    /// [`IdKind::ProcessOrStream`]). Fail-closed: [`is_readable`] treats this as readable only
    /// when *every* one of the distinct owners would independently be readable, never by picking
    /// one arm over the other.
    Ambiguous(Vec<OwnerAudience>),
}

/// Which table resolves the id to its owning process. Also the cache's key discriminator: the
/// same `Uuid` can legitimately be a `process_id` in one audience and a `stream_id`/`block_id` in
/// another (all three are client-supplied at ingestion, with no cross-table uniqueness
/// constraint), so caching by kind keeps those disjoint instead of relying on downstream
/// emptiness to make a wrong-kind hit harmless.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum IdKind {
    /// `processes.process_id`.
    Process,
    /// `blocks.block_id` -> `blocks.process_id` -> `processes`.
    Block,
    /// `list_partitions`' `view_instance_id`: either a `process_id` or a `stream_id`, resolved in
    /// one round trip. Cached under its own key rather than reusing `Process`/`Block` entries or a
    /// separate `streams` kind, so the `UNION ALL` result -- fail-closed on a collision between the
    /// two arms, see [`merge_owner_rows`] -- is what actually gets cached. A second consumer since
    /// #1486: [`AudienceGuard::authorize_view_instance`]'s scan-time check on `view_instance(...)`
    /// resolves its `view_instance_id` argument through the same kind, sharing cache entries with
    /// `list_partitions`' row filter over the same id.
    ProcessOrStream,
}

/// Default entry-count bound for [`AudienceIndex`]'s cache -- see [`super::lakehouse_context`].
/// One `Uuid` + a short audience string is roughly 100 bytes, so 100k entries is ~10 MB: a fixed
/// shape with no operational knob, mirroring `analytics-web-srv/src/data_source_cache.rs`'s
/// hardcoded `.max_capacity(1000)` rather than `MICROMEGAS_METADATA_CACHE_MB`'s per-entry-weight
/// variability.
pub const DEFAULT_AUDIENCE_CACHE_ENTRIES: u64 = 100_000;

/// Default freshness bound for [`AudienceIndex`]'s cache entries. The *only* invalidation
/// mechanism: a process's Postgres row is written once and never updated in place, but a
/// retention-then-re-export cycle can recreate the same `process_id` (deterministic UUIDv5 for
/// OTLP) under fresh `properties`, and nothing else ever expires a stale cached answer. Bounds
/// how long a re-derived process's audience can serve a stale answer.
pub const DEFAULT_AUDIENCE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

/// Merges possibly-duplicated `(id, audience)` rows into one [`OwnerAudience`] per id.
/// Fail-closed on a collision: when `ProcessOrStream`'s two arms resolve the same id to
/// *different* audiences (a `process_id`/`stream_id` collision -- both ids are client-supplied at
/// ingestion, with no cross-table uniqueness constraint), neither arm wins over the other --
/// the id maps to [`OwnerAudience::Ambiguous`], which [`is_readable`] only passes when every
/// resolved audience is independently readable. `Process`/`Block` queries never produce more than
/// one row per id, so this never triggers for them.
///
/// A `None` audience maps to [`OwnerAudience::Unknown`], not a distinct state. With
/// `owner_query_sql`'s `COALESCE` in place (#1482) a `None` is unreachable for a row that exists
/// -- a missing property resolves to the default instead -- so this arm is a safety net against a
/// bug in that expression, and it denies exactly like "no such row".
fn merge_owner_rows(rows: Vec<(Uuid, Option<String>)>) -> HashMap<Uuid, OwnerAudience> {
    let mut by_id: HashMap<Uuid, Vec<OwnerAudience>> = HashMap::new();
    for (id, audience) in rows {
        let owner = match audience {
            Some(a) => OwnerAudience::Audience(Arc::from(a)),
            None => OwnerAudience::Unknown,
        };
        let distinct_owners = by_id.entry(id).or_default();
        if !distinct_owners.contains(&owner) {
            distinct_owners.push(owner);
        }
    }
    by_id
        .into_iter()
        .map(|(id, mut owners)| {
            let owner = if owners.len() == 1 {
                owners.pop().expect("checked len() == 1 above")
            } else {
                debug!(
                    "audience_guard: '{id}' resolved to {} distinct owners across process_id/stream_id \
                     collision ({owners:?}), treating as Ambiguous (fail-closed)",
                    owners.len()
                );
                OwnerAudience::Ambiguous(owners)
            };
            (id, owner)
        })
        .collect()
}

/// The SQL shape for each [`IdKind`]. `LEFT JOIN LATERAL` (not an inner `unnest` in the `FROM`
/// list) keeps a row in the result even when its `properties` carry no `AUDIENCE_PROPERTY` --
/// the ordinary case for a process registered under a credential that carried none. `COALESCE`
/// then resolves that to the deployment's `MICROMEGAS_DEFAULT_AUDIENCE` (#1482), so such a
/// process has a real owning audience here rather than a distinct "unstamped" state. The
/// `LEFT JOIN` still matters: an inner unnest would drop the row entirely, and
/// [`merge_owner_rows`] would then report "no such id" for an id that does exist -- the same
/// verdict (deny) reached by the wrong reasoning.
///
/// `$1` is the batch of ids to resolve, bound as a `uuid[]` array so `resolve_many` is always one
/// query; `$2` is [`AUDIENCE_PROPERTY`]; `$3` is the default audience.
///
/// This site reads Postgres live, so it always resolves the *current* default -- unlike Prong A,
/// which reads whatever default was configured when the partition was materialized. The two
/// agree except across a default change that has not been followed by a regeneration pass.
fn owner_query_sql(kind: IdKind) -> &'static str {
    match kind {
        IdKind::Process => {
            "SELECT p.process_id AS id, COALESCE(a.value, $3) AS audience
             FROM processes p
             LEFT JOIN LATERAL (
                 SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1
             ) a ON TRUE
             WHERE p.process_id = ANY($1::uuid[])"
        }
        IdKind::Block => {
            "SELECT b.block_id AS id, COALESCE(a.value, $3) AS audience
             FROM blocks b
             JOIN processes p ON p.process_id = b.process_id
             LEFT JOIN LATERAL (
                 SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1
             ) a ON TRUE
             WHERE b.block_id = ANY($1::uuid[])"
        }
        IdKind::ProcessOrStream => {
            "SELECT p.process_id AS id, COALESCE(a.value, $3) AS audience
             FROM processes p
             LEFT JOIN LATERAL (
                 SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1
             ) a ON TRUE
             WHERE p.process_id = ANY($1::uuid[])
             UNION ALL
             SELECT s.stream_id AS id, COALESCE(a.value, $3) AS audience
             FROM streams s
             JOIN processes p ON p.process_id = s.process_id
             LEFT JOIN LATERAL (
                 SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1
             ) a ON TRUE
             WHERE s.stream_id = ANY($1::uuid[])"
        }
    }
}

async fn fetch_owner_rows(
    pool: &sqlx::Pool<sqlx::Postgres>,
    ids: &[Uuid],
    kind: IdKind,
    default_audience: &str,
) -> anyhow::Result<Vec<(Uuid, Option<String>)>> {
    let rows = sqlx::query(owner_query_sql(kind))
        .bind(ids)
        .bind(AUDIENCE_PROPERTY)
        .bind(default_audience)
        .fetch_all(pool)
        .await
        .with_context(|| format!("resolving owning process' audience for {kind:?}"))?;
    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").context("reading id column")?;
            let audience: Option<String> =
                row.try_get("audience").context("reading audience column")?;
            Ok((id, audience))
        })
        .collect()
}

/// Resolves *any telemetry id -> its owning process's audience*, from Postgres, cached and
/// TTL-bounded. See the module doc comment for the full rationale.
pub struct AudienceIndex {
    pool: sqlx::Pool<sqlx::Postgres>,
    cache: moka::future::Cache<(IdKind, Uuid), OwnerAudience>,
    /// The audience a never-stamped process resolves to (`MICROMEGAS_DEFAULT_AUDIENCE`, #1482),
    /// bound into every `owner_query_sql` execution. Carried from `LakehouseContext` so this
    /// prong and the materialization path share one resolved value.
    default_audience: Arc<str>,
}

impl AudienceIndex {
    pub fn new(
        pool: sqlx::Pool<sqlx::Postgres>,
        max_entries: u64,
        ttl: Duration,
        default_audience: Arc<str>,
    ) -> Self {
        let cache = moka::future::Cache::builder()
            .max_capacity(max_entries)
            .time_to_live(ttl)
            .build();
        Self {
            pool,
            cache,
            default_audience,
        }
    }

    /// Resolves a single id. A thin wrapper over [`Self::resolve_many`] -- always at least as
    /// cheap, and the batch path is where the actual query lives.
    pub async fn resolve(&self, id: Uuid, kind: IdKind) -> anyhow::Result<OwnerAudience> {
        let mut resolved = self.resolve_many(&[id], kind).await?;
        Ok(resolved.remove(&id).unwrap_or(OwnerAudience::Unknown))
    }

    /// Resolves a batch of ids of the same `kind` in at most one round trip (cache misses only).
    /// `Unknown` is never cached -- a miss may just mean "not ingested yet", and caching it would
    /// both pin a wrong answer and let a caller pollute the cache with random ids.
    pub async fn resolve_many(
        &self,
        ids: &[Uuid],
        kind: IdKind,
    ) -> anyhow::Result<HashMap<Uuid, OwnerAudience>> {
        let mut result = HashMap::with_capacity(ids.len());
        let mut misses: Vec<Uuid> = Vec::new();
        let mut seen_miss: HashSet<Uuid> = HashSet::new();
        for &id in ids {
            if let Some(cached) = self.cache.get(&(kind, id)).await {
                result.insert(id, cached);
            } else if seen_miss.insert(id) {
                misses.push(id);
            }
        }
        if misses.is_empty() {
            return Ok(result);
        }
        let rows = fetch_owner_rows(&self.pool, &misses, kind, &self.default_audience).await?;
        let resolved = merge_owner_rows(rows);
        for id in misses {
            let owner = resolved.get(&id).cloned().unwrap_or(OwnerAudience::Unknown);
            if owner != OwnerAudience::Unknown {
                self.cache.insert((kind, id), owner.clone()).await;
            }
            result.insert(id, owner);
        }
        imetric!(
            "audience_cache_entry_count",
            "count",
            self.cache.entry_count()
        );
        Ok(result)
    }
}

impl std::fmt::Debug for AudienceIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudienceIndex")
            .field("entries", &self.cache.entry_count())
            .finish()
    }
}

/// The uniform, existence-oracle-proof denial text every Prong B guard returns:
/// `{fname}: '{id}' not found or not accessible`. Extracted so [`AudienceGuard::authorize`] and
/// [`AudienceGuard::authorize_view_instance`]'s fail-closed fallthrough rule can't drift apart.
fn not_found_err<T>(fname: &str, id: &str) -> datafusion::error::Result<T> {
    plan_err!("{fname}: '{id}' not found or not accessible")
}

/// Pure, offline-testable: the whole authorization rule, with no I/O in it. `ReadScope::All` is
/// the only branch that passes `Unknown` -- every other combination denies it unconditionally.
pub fn is_readable(scope: &ReadScope, owner: &OwnerAudience) -> bool {
    match scope {
        ReadScope::All => true,
        ReadScope::Audiences(auds) => match owner {
            OwnerAudience::Unknown => false,
            OwnerAudience::Audience(a) => auds.iter().any(|x| x.as_str() == &**a),
            OwnerAudience::Ambiguous(owners) => {
                !owners.is_empty() && owners.iter().all(|owner| is_readable(scope, owner))
            }
        },
    }
}

/// Witness that some id's owning process was resolved and found readable under the caller's
/// scope. Constructible only by [`AudienceGuard::authorize`] -- a future call site that wants an
/// inner, unscoped session must hold one of these first, which is what keeps a fourth such UDTF
/// from silently reproducing the bypass this stage closes by calling `CallerContext::internal()`
/// directly. Not airtight: `internal()` stays public for genuinely non-user-reachable callers.
#[derive(Debug)]
pub struct Authorized {
    id: Uuid,
}

impl Authorized {
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// The `CallerContext` an inner, post-authorization session may run under. Deliberately
    /// `ReadScope::All` (not the caller's own scope) -- see
    /// `tasks/1371_udtf_udf_guards_plan.md` §6 for why guard-then-internal is the chosen shape
    /// here rather than scope inheritance: every inner statement these three call sites run is
    /// server-constructed and confined to `self.id`, so if `self.id`'s process is readable,
    /// everything those statements can reach is readable too.
    pub fn internal_caller(&self) -> CallerContext {
        CallerContext::internal()
    }
}

/// The whole guard: pure decision (`is_readable`) plus async resolution (`AudienceIndex`),
/// fail-closed throughout. One instance is built per request (in `query.rs`'s
/// `register_lakehouse_functions`) and shared, via `Arc`, across every arg-addressed UDTF/UDF the
/// request's session registers.
#[derive(Debug)]
pub struct AudienceGuard {
    read_scope: ReadScope,
    /// Whether this deployment's caller passes the lakehouse admin gate --
    /// `caller.is_admin || !caller.admin_principal_possible` (`query.rs`), the same boolean that
    /// already governs registration of the mutating lakehouse UDTFs/UDFs. Occupies the slot the
    /// removed `unstamped_audience` field used to (#1482 §4): it is what
    /// [`Self::global_rows_visible`] now consults instead.
    lakehouse_admin: bool,
    public_view_sets: Vec<String>,
    index: Arc<AudienceIndex>,
}

impl AudienceGuard {
    pub fn new(
        read_scope: ReadScope,
        lakehouse_admin: bool,
        public_view_sets: Vec<String>,
        index: Arc<AudienceIndex>,
    ) -> Self {
        Self {
            read_scope,
            lakehouse_admin,
            public_view_sets,
            index,
        }
    }

    /// The caller's scope. Exposed so callers with their own row-level filtering
    /// (`list_partitions`) can decide up front whether to even attempt an optimization (e.g.
    /// `LIMIT` pushdown) that's only valid under `ReadScope::All`.
    pub fn read_scope(&self) -> &ReadScope {
        &self.read_scope
    }

    /// `ReadScope::All` ⇒ `Ok` with no I/O at all. Otherwise resolves `id`'s owning process's
    /// audience and denies with a uniform, existence-oracle-proof message
    /// (`{fname}: '{id}' not found or not accessible`) on anything but a readable verdict -- a
    /// resolution error included, never a pass. The real reason (resolved `OwnerAudience`, the
    /// caller's scope) is logged at `debug!`, for operators only.
    pub async fn authorize(
        &self,
        id: Uuid,
        kind: IdKind,
        fname: &str,
    ) -> datafusion::error::Result<Authorized> {
        if self.read_scope == ReadScope::All {
            return Ok(Authorized { id });
        }
        let owner = match self.index.resolve(id, kind).await {
            Ok(owner) => owner,
            Err(e) => {
                debug!("{fname}: audience resolution failed for '{id}' ({kind:?}): {e:#}");
                return Err(DataFusionError::External(e.into()));
            }
        };
        if is_readable(&self.read_scope, &owner) {
            Ok(Authorized { id })
        } else {
            debug!(
                "{fname}: denying '{id}' ({kind:?}): owner={owner:?}, scope={:?}",
                self.read_scope
            );
            not_found_err(fname, &id.to_string())
        }
    }

    /// Batch form of the same rule, for the callers that row-filter (`list_partitions`) or need
    /// an all-or-nothing verdict over a batch (`get_payload`) rather than a single witness.
    /// `ReadScope::All` ⇒ every id is readable, with no I/O.
    pub async fn readable_ids(
        &self,
        ids: &[Uuid],
        kind: IdKind,
    ) -> datafusion::error::Result<HashSet<Uuid>> {
        if self.read_scope == ReadScope::All {
            return Ok(ids.iter().copied().collect());
        }
        let owners = self
            .index
            .resolve_many(ids, kind)
            .await
            .map_err(|e| DataFusionError::External(e.into()))?;
        Ok(ids
            .iter()
            .copied()
            .filter(|id| {
                let owner = owners.get(id).unwrap_or(&OwnerAudience::Unknown);
                is_readable(&self.read_scope, owner)
            })
            .collect())
    }

    /// Whether `view_set_name` is on the `MICROMEGAS_PUBLIC_VIEW_SETS` allowlist -- an operator
    /// declaring every row of a view set readable by any authenticated caller. Shared by
    /// [`Self::global_rows_visible`] and [`Self::authorize_view_instance`]'s rule (2) so the two
    /// read the same allowlist through one accessor.
    pub fn is_public_view_set(&self, view_set_name: &str) -> bool {
        self.public_view_sets.iter().any(|s| s == view_set_name)
    }

    /// `list_partitions`' `'global'`-row rule (#1482 §4): a global partition is a multi-audience
    /// file -- it has no single owning audience to check against the caller's scope. Visible
    /// under `ReadScope::All`, when `view_set_name` is on the public allowlist, or when the
    /// caller passes the lakehouse admin gate (the same boolean that already governs the
    /// mutating UDTFs/UDFs: a caller who can `retire_partitions`/`regenerate_partitions` a global
    /// file can see it -- no new authority, no new knob). Previously: visible whenever the
    /// removed `unstamped_audience` knob was itself in the caller's scope.
    pub fn global_rows_visible(&self, view_set_name: &str) -> bool {
        match &self.read_scope {
            ReadScope::All => true,
            ReadScope::Audiences(_) => {
                self.is_public_view_set(view_set_name) || self.lakehouse_admin
            }
        }
    }

    /// The `view_instance(view_set_name, view_instance_id)` scan-time check (#1486), run by
    /// `MaterializedView::scan` before `jit_update` -- so a caller scoped to one audience cannot
    /// trigger JIT materialization of an instance it cannot read.
    ///
    /// Rules, in order:
    ///
    /// 1. `ReadScope::All` -> `Ok(())`, no I/O. Internal, maintenance, `--disable-auth`, and
    ///    every inner session built from an `Authorized::internal_caller()` take this arm.
    /// 2. `view_set_name` on `public_view_sets` -> `Ok(())`. Matches Prong A: an operator who
    ///    declared a view set public has said every row of it is readable, so denying
    ///    materialization of one of its instances would be incoherent. Confidentiality-only:
    ///    this leaves the cost/availability residual this method exists to close fully open for
    ///    any view set an operator puts on the allowlist -- any authenticated caller can still
    ///    force `jit_update` for arbitrary process/stream ids of a public view set. Deliberate.
    /// 3. `view_instance_id == "global"` -> `Ok(())`. Global instances are row-filtered, not
    ///    call-guarded: `jit_update` is a no-op for every `'global'` instance today
    ///    (`LogView`/`MetricsView`, the only two view sets that accept `'global'`), so there is
    ///    no materialization to protect against, and denying the call would break the legal
    ///    `view_instance('log_entries', 'global')` spelling of `SELECT * FROM log_entries` for no
    ///    security gain -- Prong A already filters its rows one at a time. This is a different
    ///    rule from `list_partitions`' `global_rows_visible`, which gates *visibility of
    ///    partition metadata* about a `'global'` file with no per-row filter available; reusing
    ///    it here would deny the `log_entries`/`measures` `'global'` instances to every
    ///    non-admin scoped caller, a regression. The invariant that makes this rule sound --
    ///    `'global'` never triggers JIT -- lives in each view set's `jit_update` impl; a future
    ///    view set whose `'global'` instance does materialize would need to revisit this rule.
    /// 4. `view_instance_id` parses as a `Uuid` -> [`Self::authorize`] under
    ///    `IdKind::ProcessOrStream`, discarding the returned `Authorized`. The witness exists to
    ///    gate construction of an inner unscoped session; this call site builds none --
    ///    `jit_update`'s `CallerContext::internal()` is unchanged and stays where it is.
    /// 5. anything else -> the same uniform denial as (4). Fail-closed, matching
    ///    `list_partitions`'s "anything else is dropped" rule. Unreachable for today's view sets
    ///    (every `make_view` either accepts `'global'` or `Uuid::parse_str`s, failing at plan
    ///    time otherwise), so this is a safety net against a future view set with a different id
    ///    vocabulary.
    pub async fn authorize_view_instance(
        &self,
        view_set_name: &str,
        view_instance_id: &str,
    ) -> datafusion::error::Result<()> {
        if self.read_scope == ReadScope::All {
            return Ok(());
        }
        if self.is_public_view_set(view_set_name) {
            return Ok(());
        }
        if view_instance_id == "global" {
            return Ok(());
        }
        if let Ok(id) = Uuid::parse_str(view_instance_id) {
            self.authorize(id, IdKind::ProcessOrStream, "view_instance")
                .await?;
            return Ok(());
        }
        not_found_err("view_instance", view_instance_id)
    }
}
