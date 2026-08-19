# Admin-Managed Query Deny List Plan

Issue: [#1488](https://github.com/madesroches/micromegas/issues/1488)

## Overview

A single misbehaving client — a dashboard on a short refresh interval, an alert rule that re-fires
on failure, a notebook cell in a retry loop — can keep a heavy query in flight continuously and
saturate `flight-sql-srv`. Today the only remedies are restarting the service (the offender comes
back on its next tick) or revoking the caller's credentials (too blunt for a shared service
account). This plan adds a deny list of admin-managed rules, stored in Postgres, cached in every
`flight-sql-srv` replica, and evaluated at the front of `execute_query` — before the session
context is built and before planning — so a matching query is rejected for a few microseconds of
work instead of a memory-pool reservation and a wave of object-store reads.

Rules carry a reason and their creator, and stay in force until an admin removes them explicitly.
They are administered from SQL (`list_query_denials()` / `deny_queries(...)` / `remove_query_denial(...)`,
admin-gated exactly like `retire_partitions`) and from a new **Admin → Query Deny List** screen in the
analytics web app that drives those same SQL functions.

This is the manual valve an on-call admin can pull without a deploy. Rate limiting, per-user
concurrency caps, and cost-based admission control remain out of scope (separate issues).

## Current State

### Query path

`FlightSqlServiceImpl::execute_query` (`rust/public/src/servers/flight_sql_service_impl.rs:591`)
runs, in order:

1. mint `query_id`, parse SQL from the ticket, parse `query_range_begin`/`end` headers (lines 601-640)
2. `validate_and_resolve_user_attribution_grpc(metadata)` → `attr` (line 643)
3. read `x-client-type` / `-agent` / `-entrypoint` / `-session` / `-notebook` / `-cell` (lines 645-659)
4. build `ScopedMemoryPool` and `QueryAuditState` (lines 686-718) — **the first point where all
   attribution is assembled**
5. `scoped_runtime` → `caller_context` → `make_session_context` (lines 724-744) — session context,
   view registration, `OwnershipRewrite`
6. `ctx.sql(sql)` (planning), optional `limit`, physical plan, `execute_stream`, Flight encoding

Steps 5-6 are where the money goes: JIT partition materialization, object-store reads, memory-pool
reservations. Step 4 is the natural insertion point for the check.

`do_get_statement` (line 1036) and `do_get_fallback` (line 866) are the only callers of
`execute_query`. `do_action_create_prepared_statement` (line 1235) builds a session context and
plans without executing, and `do_get_prepared_statement` (line 1046) is `api_entry_not_implemented!()`
— so every executed query reaches `execute_query`, prepared or not (§5).

### Audit record

`QueryAuditRecord` (`rust/public/src/servers/query_audit.rs:79`) already carries every field a rule
would want to match on: `client_ip`, `client`, `agent`, `entrypoint`, `session`, `notebook`, `cell`,
`user`, `email`, `service_account_name`, and the raw `sql`. It is emitted as one JSON line under the
`flightsql_query_audit` target on every terminal path — success, failure, and abandoned-mid-drain
(`QueryAuditState::emit`, `fail`, and its `Drop` impl — all in
`rust/public/src/servers/flight_sql_service_impl.rs:314,369,406`). It does **not** currently
carry a normalized-SQL fingerprint.

`error_class` is derived from the gRPC code by `error_class()` (`flight_sql_service_impl.rs:117`): `InvalidArgument` /
`Unimplemented` → `"user"`, `ResourceExhausted` → `"resource"`, everything else → `"internal"`.

### Admin gating

`register_lakehouse_functions` (`rust/analytics/src/lakehouse/query.rs:176-201`) registers the
mutating functions (`retire_partitions`, `materialize_partitions`, `regenerate_partitions`,
`retire_partition_by_file`, `retire_partition_by_metadata`) only when
`caller.is_admin || !caller.admin_principal_possible`. `CallerContext`
(`rust/analytics/src/lakehouse/read_scope.rs`) carries `read_scope`, `is_admin`,
`isolation_config`, `admin_principal_possible` — **no caller identity** (no user id / email).

`is_admin` comes from the `is_admin` gRPC header, stamped by `AuthService`
(`rust/auth/src/tower.rs:102-139`) from `AuthContext.is_admin` with client-supplied copies
stripped; absent header means trusted admin (`--disable-auth`). API keys hardcode
`is_admin: false` (`rust/auth/src/db_api_key.rs:350`), so a dashboard or alert service account can
never be an admin principal.

### Postgres and caching precedents

- Lakehouse schema migrations: `rust/analytics/src/lakehouse/migration.rs`,
  `LATEST_LAKEHOUSE_SCHEMA_VERSION = 8`. `LakehouseContext::from_connection`/`from_env` run
  `migrate_lakehouse` at startup (`lakehouse_context.rs:45,60`), so **`flight-sql-srv` itself
  migrates on boot**.
- `AudienceIndex` (`rust/analytics/src/lakehouse/audience_guard.rs:202`) is the precedent for a
  Postgres-backed, TTL-cached lookup owned by `LakehouseContext` and reached from `query.rs` via
  `lakehouse.audience_index()`.
- Row-returning UDTF over Postgres: `list_partitions_table_function.rs`. Mutating UDTF:
  `retire_partitions_table_function.rs` (streams a log via `LogStreamTableProvider`).
- `sqlparser` 0.62 (with the `visitor` feature) is reachable as `datafusion::sql::sqlparser`;
  `sha2` is a workspace dependency, declared in `analytics/Cargo.toml` as `sha2.workspace = true`
  like every other non-dev dependency of that crate.

### Web app

- `AdminPage.tsx` is a card grid of admin destinations, each `AuthGuard requireAdmin`.
- `components/ApiKeysAdminPage.tsx` is the closest full-page precedent (table + create dialog +
  `ConfirmDialog` + `ErrorBanner`) — a shared component, not a route: `routes/AnalyticsApiKeysPage.tsx`
  and `routes/IngestionApiKeysPage.tsx` both configure it. It talks to `analytics-web-srv` REST routes
  backed by the telemetry DB, which is the part this screen does *not* copy (§9).
- SQL-driven pages (`ProcessesPage.tsx`, `ProcessLogPage.tsx`, …) use `useStreamQuery` →
  `POST /api/query-stream` (`rust/analytics-web-srv/src/stream_query.rs`), which forwards the
  browser user's own bearer token to FlightSQL via `BearerFlightSQLClientFactory`. So an admin's
  web query resolves `is_admin: true` at `flight-sql-srv` and sees the admin functions.
- `stream_query.rs:90` holds `BLOCKED_FUNCTIONS`, a substring blocklist that refuses
  `retire_partitions*` in web queries.

## Design

### 1. Storage — `query_deny_list` table (lakehouse migration v9)

```sql
CREATE TABLE query_deny_list (
  rule_id      UUID PRIMARY KEY,
  created_at   TIMESTAMPTZ  NOT NULL,
  created_by   VARCHAR(255) NOT NULL,
  reason       TEXT         NOT NULL,
  -- a boolean SQL expression over the match context (§3)
  match_expr   TEXT         NOT NULL,
  -- NULL until the rule first fires; the "is this rule still doing anything?" signal
  last_hit_at  TIMESTAMPTZ
);
```

One `match_expr` column, not a column per matcher. A fixed column set fossilizes the matching
language into the schema: every new attribute is a migration, and the only combinator it can ever
express is AND. A single expression column carries an arbitrary predicate today and can grow
richer semantics later without touching the schema at all — the evolution path §3 describes.

`last_hit_at` is flushed on the refresh tick (§4), and matters more here than it would with expiring
rules: with rules standing until removed, "last fired three weeks ago" is what tells an operator a
rule is stale and safe to remove, and "last fired four seconds ago" is what tells them the offender
has not been fixed.

There is deliberately **no `hit_count`**. A per-rule denial count is already emitted as the
`query_denied` metric (§6), tagged with the rule id, at full time resolution and with history — a
stored counter would be a strictly worse copy of a signal the deployment already has, and it would
cost a column, an atomic, half the flush `UPDATE`, and a test to keep in sync. `last_hit_at` is not
redundant with the metric in the same way: it is the one thing `list_query_denials()` must answer
without a second query against `measures`.

No expiry column: a rule is in force from insertion until `remove_query_denial` deletes it. The
table holds at most `MICROMEGAS_QUERY_DENY_MAX_RULES` rows (§10) and needs no index — every replica
reads all of it on each refresh tick.

`LATEST_LAKEHOUSE_SCHEMA_VERSION` 8 → 9, `upgrade_v8_to_v9`. No `SCHEMA_VERSION` (partition
file-schema) change: no partition content is affected.

### 2. Normalized SQL fingerprint

New in `rust/analytics/src/lakehouse/query_deny_list.rs`:

```rust
/// Literal-stripped fingerprint of a statement: the first 16 hex chars of the
/// SHA-256 of the normalized token stream. Tokenizes internally; the token stream is an
/// implementation detail and does not appear in the signature.
pub fn fingerprint_of(sql: &str) -> String;
```

Implementation: `sqlparser::tokenizer::Tokenizer` over `GenericDialect`, then

- every `Token::Number`, `SingleQuotedString`, `NationalStringLiteral`, `HexStringLiteral`,
  `EscapedStringLiteral`, `TripleSingleQuotedString`, `TripleDoubleQuotedString` → `?`
- whitespace runs collapse to one space; comments dropped
- `Token::Word` is lowercased only when `quote_style.is_none()` — keywords and unquoted identifiers
  fold to lowercase (identifiers are already case-insensitive in DataFusion unless quoted), but a
  quoted word is kept verbatim, case included. This, not a separate `DoubleQuotedString` carve-out,
  is what protects this product's dotted column names: under `GenericDialect`, a double-quoted
  identifier like `b."processes.exe"` tokenizes as `Token::Word { quote_style: Some('"') }`, not
  `Token::DoubleQuotedString` (the tokenizer's double-quote arm is gated on
  `is_delimited_identifier_start`, which `GenericDialect` sets true for `"`). Lowercasing on
  `quote_style` rather than stripping a `DoubleQuotedString` token that never actually appears here
  is what keeps `"processes.exe"` and `"processes.username"` from collapsing into the same
  fingerprint and denying a query never meant to match.
- tokens joined with a single space, then SHA-256, hex, truncated to 16 chars (64 bits)

Tokenization failure falls back to hashing the whitespace-collapsed raw text, so a fingerprint
always exists.

This is what makes the dashboard case work: consecutive refreshes differ only in their time-range
literals, so they collapse to one fingerprint. A 64-bit fingerprint's collision odds across a
realistic query population are negligible, and the consequence of a collision is bounded by the
rule's other match predicates.

The fingerprint is computed once per query in `execute_query` and stored on `QueryAuditState`, so
it costs nothing extra to also emit it in the audit record (§7) — which is how an operator gets the
value to paste into `deny_queries`. Unlike the deny-list check itself, this cost is **not**
conditional on any rule existing: `QueryAuditState::emit` writes `sql_hash` on every terminal path
(success, failure, and abandoned-mid-drain), so `fingerprint_of` runs on every query regardless of
whether the deny list is empty. That cost is accounted for in §3 rather than assumed away.

### 3. Match expression

#### Prior art

"Express a predicate over request attributes, store it, evaluate it safely" is a well-trodden
problem, with a few established answers:

| Approach | Where it's used | Fit here |
|---|---|---|
| **CEL** (Common Expression Language) | Kubernetes admission policies & CRD validation, Envoy RBAC and rate-limit matching, Istio | The closest thing to a standard for exactly this job — typed, non-Turing-complete, bounded evaluation. Rust support exists (`cel-interpreter`), but it means a new dependency and a second expression language in a product whose interface is already SQL. |
| **OPA / Rego** | Cluster-wide authorization policy | A whole policy language and runtime (`regorus` in Rust). Far past what an incident valve needs. |
| **Envoy Unified Matcher API** | Envoy xDS | A protobuf matcher *tree*, not a text language. Useful as a shape reference (predicate tree over typed inputs); nothing to adopt directly. |
| **JsonLogic** | Assorted rules engines | JSON-encoded predicate trees; trivial to build a form UI over and to serialize. Not really a standard, and painful to write by hand. |
| **A boolean SQL expression** | — | Not a "matcher standard", but a standard *language* — already this product's stable interface, already parsed by an engine in the process, and already what an admin reading the audit log thinks in. |

**This plan uses a boolean SQL expression**, parsed *and evaluated* by DataFusion. No new language
for the admin to learn and no grammar of ours to specify, and no evaluator of ours to get subtly
wrong. CEL becomes the better answer the day the matcher has to be
authored by non-admins, or evaluated somewhere with no DataFusion to parse it — and that is the
trigger to revisit this.

#### The match context

Every rule is a predicate over one fixed, documented schema (`match_schema()`, a `DFSchema` of
nullable `Utf8` fields) — the attributes `execute_query` has already resolved by the time the check
runs:

| Column | NULL when |
|---|---|
| `user_id`, `email` | never — `user_id` is the *authenticated* OIDC subject only for a non-delegating caller under OIDC. A delegating service account (`x-allow-delegation: true` plus a client-supplied `x-user-id`/`x-user-email`) puts its own authenticated identity in `service_account` and echoes the client-asserted id into `user_id` instead; with no `x-auth-subject` at all (`--disable-auth`), `user_id`/`email` fall back to the client-supplied headers, defaulting to `'unknown'`. So a rule keyed on `user_id`/`email` targets a client-asserted value in the delegating-service-account case, and can be evaded by changing a header |
| `service_account` | the caller is a service account that is **not delegating** — i.e. also NULL for an ordinary human caller. It is set only when a service account calls on behalf of a user (`x-user-id`/`x-user-email` present alongside its own credentials); a non-delegating service account's identity is in `user_id` instead, so a rule meant to target one should match on `user_id`, not `service_account` |
| `client`, `agent`, `entrypoint` | never (`'unknown'` when the header is absent) |
| `session`, `notebook`, `cell` | the caller sent no such header |
| `client_ip` | never |
| `sql` | never — the raw statement text |
| `sql_hash` | never — the normalized fingerprint (§2) |

Server-derived, not client-influenceable: `client_ip`, `sql`, `sql_hash`, `service_account`.
Client-asserted, and therefore something an offender can change to evade a rule: `user_id`,
`email`, `client`, `agent`, `entrypoint`, `session`, `notebook`, `cell`.

The identity column is named `user_id`, not `user`: DataFusion's default (`Generic`) SQL dialect
parses a bare `user` in expression position as the zero-argument function call `user()`, not a
column reference, and no such scalar function is registered — `user = 'jean'` would fail at
planning with "Invalid function 'user'". None of the other columns collide with a reserved word.

Every attribute is a string, so there are no coercion surprises. Adding an attribute later means
appending one field to this schema: existing expressions keep compiling, and no migration is
involved. That is the point of the single-column design.

NULL semantics come from SQL and are the ones we want: `notebook = 'fleet-overview'` evaluates to
NULL — not true — for a query that carried no notebook header, so the rule does not fire.

`user_id` and `email` match exactly, case-sensitive — `=` with no case folding — matching the existing
precedent at `OidcProvider::is_admin` (`rust/auth/src/oidc.rs`), which already compares admin
identities against the configured list with plain `==` and has no case-insensitive path anywhere in
`rust/auth/src`. This is not a new decision, just the same rule applied here.

#### Measured: what evaluation costs

This check runs in front of **every** query, for as long as a rule stands, so it was measured rather
than argued about. Release build, one rule set evaluated against one query's attribution (12 string
attributes), development machine: DataFusion `PhysicalExpr` on a one-row `RecordBatch` costs
**~3.4 µs at one rule, ~6.2 µs at ten, and ~45 µs at the 100-rule cap** (§10). Most of the
single-rule figure — 2.8 µs — is building the one-row batch; only ~400 ns per rule is evaluation.
Entering Arrow at all is the cost.

That is affordable here. The check sits immediately in front of a phase the server already measures
in *milliseconds*: `make_session_context` registers ~16 UDF/UDTFs and awaits a `register_table` per
global view, and the audit record's own `context_init_ms` field is there to track it. And `check`
returns immediately on an empty snapshot, so the cost is only paid while a rule stands — the steady
state of a deployment that is not mid-incident is zero. A compiled evaluator of our own would be
much faster (throwaway prototypes measured in tens of nanoseconds), and it remains the escape hatch
behind an unchanged `check` signature if a profile ever justifies it; until then it is not worth
owning Kleene logic and `LIKE` lowering.

What is **not** conditional on a rule existing is the fingerprint: `fingerprint_of` costs ~1.2 µs on
a ~130-character statement and runs on every query, because `QueryAuditRecord` carries `sql_hash` on
every terminal path (§7). "Zero rules cost nothing" is true of the evaluator but not of the feature.
That cost is accepted because the audit log needs the fingerprint for the "paste it into
`deny_queries`" workflow whether or not the deny list is in active use.

#### No anchor requirement

An earlier version of this section required every rule to carry a top-level `column = 'literal'`
conjunct, so that a query disagreeing on that field could skip the rule after one hash probe. That
requirement **forbids legitimate rules**: `sql LIKE '%thread_spans%' OR client_ip = '10.4.9.221'` —
"stop anything scanning thread_spans, and anything from that host" — is exactly what an on-call
admin reaches for, and a top-level disjunction has no conjunct to anchor *on*. Splitting it in two
does not help either, since `sql LIKE '%thread_spans%'` alone is unanchorable by construction. A
blanket "nothing may scan this view right now" is one of the strongest levers the deny list offers,
and forbidding it to save microseconds is the wrong trade. So: **any boolean expression over the
match context is accepted, anchored or not.**

The only *shape* rejected on principle is one with **no column reference at all** (`true`,
`1 = 1`) — a rule that would deny every query in the deployment. That is a semantic guard, and
conflating it with a performance guard is what produced the mistaken anchor rule in the first place.

#### The design that follows

Everything expensive happens at refresh, since rules are few and static:

1. **Parse** with `ctx.parse_sql_expr(match_expr, &match_schema())`. `ctx` is a bare
   `SessionContext::new()` held on `QueryDenyList` for exactly this call — no lakehouse tables or
   catalog registered against it, just DataFusion's parser and name resolution, so there is no
   grammar of ours to specify or keep in sync. `compile_match_expr(ctx: &SessionContext, match_expr:
   &str) -> Result<Arc<dyn PhysicalExpr>>` is the function that owns all three steps; `refresh`
   calls it with `&self.ctx` on every reload, and `deny_queries`'s `call_with_args` calls it the
   same way, still synchronously, before `insert` ever runs (§8).
2. **Validate** with exactly two checks: the result type is `Boolean`, and at least one column is
   referenced. Nothing else is enumerated. An unknown column or function already fails at step 1,
   and a subquery, aggregate, or window function cannot be lowered to a scalar `PhysicalExpr` and
   fails at step 3 — each with DataFusion's own diagnostic, which is a better message than one of
   ours. Rules are authored by admins, the same people who can call `retire_partitions`, so
   validation is not a trust boundary: it catches the two mistakes that would otherwise be silent,
   a rule that means nothing (non-boolean) and a rule that denies everything (no column reference).
   Everything else is the admin's business, including arithmetic and column-to-column comparison,
   which are harmless over a one-row all-`Utf8` batch and cost a visitor arm each to forbid.
3. **Plan** the validated `Expr` into an `Arc<dyn PhysicalExpr>` with `create_physical_expr` against
   `match_schema()`, so the per-query path never touches the planner. Closest precedent in the crate:
   `analytics/src/dfext/predicate.rs:21`, which already does `state.create_physical_expr(expr,
   &df_schema)` for the same reason.

   **This path runs no type-coercion pass.** Neither `parse_sql_expr` nor `create_physical_expr`
   applies the analyzer's `TypeCoercion` rule, so an expression that would need an inserted cast
   fails to plan instead of being quietly coerced. Over an all-`Utf8` match context compared against
   string literals that is exactly the behavior we want — a type mismatch (`client = 42`,
   `notebook = now()`) becomes a compile error the admin sees at `deny_queries` time, rather than a
   rule that silently never fires. But it makes the accepted subset a property of DataFusion's
   *physical* planner rather than of anything we wrote, so the subset is pinned by tests (Testing
   Strategy, "No coercion pass"). If some expression that ought to be accepted turns out to need a
   cast, the fix is to run the `Expr` through the `TypeCoercion` analyzer rule here in step 3 — one
   place, applied identically on every replica — not to widen or hand-code the accepted subset.

Rules are held in one slice ordered by `(created_at, rule_id)`, oldest first. Per query, `check`
does: empty snapshot → return; otherwise build a one-row `RecordBatch` from the borrowed attribution
and evaluate each rule's `PhysicalExpr` in that order, returning the first rule that evaluates to
true. Every replica orders the rules identically, so "oldest matching rule wins" (§4) is simply the
first match, and two replicas name the same rule for the same query.

#### Grammar

DataFusion parses and evaluates, so the syntax is SQL's. The useful subset over a one-row match
context is `AND`/`OR`/`NOT`, `=`/`!=`, `IN`, `LIKE`/`ILIKE`, `IS [NOT] NULL`, and `regexp_like`, over
the match-context columns and string literals — but that subset is documentation, not a code path.
Nothing enumerates it: an expression outside it either fails to parse, fails to plan, or works.

The two expressions rejected on principle are the ones the parser and planner accept but that mean
something the admin did not intend: one whose result is not `Boolean`, and one with no column
reference at all (`true`, `1 = 1`), which would deny every query in the deployment. There is no
anchor requirement: any boolean shape, including a top-level `OR`, is accepted (§3).

Non-deterministic functions are not forbidden either, and the reach of forbidding them would have
been close to nil: `now()` returns a `Timestamp` and cannot be compared to a `Utf8` match-context
column at all, and a `random()`-only expression references no column and is already rejected. What
survives — `sql LIKE '%x%' AND random() < 0.1`, a sampling rule — makes a rule fire on some replicas
and not others, which is what an admin who writes it asked for.

Regex is safe: the `regex` crate behind DataFusion's `regexp_like` does not backtrack and
guarantees linear-time matching. (This corrects an earlier version of this plan, which excluded
regex on ReDoS grounds — a hazard of backtracking engines, not of this one.)

Examples, all valid:

```sql
sql_hash = '9f2c41ab73de0155' AND entrypoint = 'grafana-alert'
user_id = 'dashboards-svc' AND notebook = 'fleet-overview'
client_ip = '10.4.9.221' AND sql LIKE '%thread_spans%'
client = 'grafana' AND regexp_like(sql, '(?i)from\s+view_instance')
email = 'jean@example.com' AND (notebook IS NOT NULL OR entrypoint = 'notebook')
```

A blanket rule with no equality to key on is equally valid, and is the strongest lever the deny
list offers during an incident:

```sql
sql LIKE '%thread_spans%' OR client_ip = '10.4.9.221'
sql LIKE '%thread_spans%'
```

#### Where this can go next

The column holds text and the expression language is DataFusion's, so richer semantics land without
a migration and mostly without code: numeric comparison the moment the match context carries a
numeric attribute, a structured `deny_queries` variant that renders an expression for a UI builder,
cost predicates once an estimate is available at check time, or a `test_query_denial(expr)` function
that dry-runs an expression against recent audit records before it goes live. Performance is an
independent axis: an evaluator of our own can replace DataFusion behind the same `check` signature
without touching the stored form (§3).

### 4. Rule model, evaluation, and cache

```rust
pub struct QueryAttribution<'a> {      // borrowed view of what execute_query already resolved
    pub user_id: &'a str, pub email: &'a str,
    pub service_account: Option<&'a str>,
    pub client: &'a str, pub agent: &'a str, pub entrypoint: &'a str,
    pub session: Option<&'a str>, pub notebook: Option<&'a str>, pub cell: Option<&'a str>,
    pub client_ip: &'a str,
    pub sql: &'a str, pub sql_hash: &'a str,
}

impl QueryAttribution<'_> {
    /// One-row `RecordBatch` over `match_schema()`, in column order -- the only thing `check`
    /// builds per query, and the only allocation on the path.
    fn to_batch(&self) -> RecordBatch;
}

/// The DB row, exactly as `list_query_denials()` returns it (§8) and as `insert` echoes back.
/// No compiled expression or in-process timestamp here — those exist only for a rule that has been
/// compiled into a `DenySnapshot`, and a freshly inserted or listed row has not necessarily
/// been through that yet on every replica.
pub struct QueryDenyRow {
    pub rule_id: Uuid, pub created_at: DateTime<Utc>, pub created_by: String,
    pub reason: String, pub match_expr: String,
    pub last_hit_at: Option<DateTime<Utc>>,   // None until the rule first fires
}

/// A row compiled into one snapshot: the row, its planned expression, and the one piece of
/// in-process state a rule accumulates. Lives only inside `DenySnapshot`.
pub struct QueryDenyRule {
    pub row: QueryDenyRow,
    expr: Arc<dyn PhysicalExpr>,          // planned once at refresh, evaluated per query (§3)
    last_hit: AtomicI64,                  // unix seconds, 0 = not hit since the last flush
}

/// The compiled rules, ordered by `(created_at, rule_id)`, oldest first. Rebuilt wholesale on every
/// refresh, which is cheap because rules are few and change rarely.
///
/// An alias, not a newtype: nothing hangs behind it. `check` and `refresh` are methods on
/// `QueryDenyList`, the ordering invariant is established by the one `ORDER BY` in `refresh` rather
/// than enforced by a constructor, and the slice is never handed out — so a wrapper struct would add
/// a field access at every use and nothing else. `Arc<[_]>` rather than `Arc<Vec<_>>` for the same
/// reason `ReadScope::Audiences` holds `Arc<[String]>`: the snapshot is immutable once built, and
/// this drops a pointer hop off the per-query path. If a snapshot ever grows a lookup structure (an
/// index by fingerprint, say), that is the point to promote it to a struct.
pub type DenySnapshot = Arc<[Arc<QueryDenyRule>]>;

pub struct QueryDenyList {               // owned by LakehouseContext, like AudienceIndex
    pool: sqlx::Pool<sqlx::Postgres>,
    // Bare `SessionContext::new()` — no lakehouse tables or catalog registered, held only so
    // `compile_match_expr` can call `ctx.parse_sql_expr(expr, &match_schema())`; both `refresh`
    // and `insert` compile through it.
    ctx: SessionContext,
    snapshot: std::sync::RwLock<DenySnapshot>,  // `arc-swap` is not a workspace dep;
                                   // a read lock held for one clone is enough
}

impl QueryDenyList {
    pub fn check(&self, q: &QueryAttribution<'_>) -> Option<Arc<QueryDenyRule>>;
    pub async fn refresh(&self) -> Result<()>;     // flush `last_hit`, reload + recompile
    // `compiled` was already produced by `call_with_args`'s synchronous `compile_match_expr`
    // (§8); `insert` stores `match_expr` verbatim and does not re-validate it.
    pub async fn insert(&self, match_expr: &str, compiled: Arc<dyn PhysicalExpr>, reason: &str,
                        created_by: &str) -> Result<QueryDenyRow>;
    pub async fn delete(&self, rule_id: Uuid) -> Result<bool>;
    pub async fn list(&self) -> Result<Vec<QueryDenyRow>>;
    pub fn spawn_refresh_task(self: Arc<Self>, shutdown: impl Future<Output = ()> + Send + 'static);
}
```

- **Zero rules make `check` itself free.** `check` returns on an empty-snapshot test — the steady
  state of every deployment that is not mid-incident. This is a claim about `check`, not about the
  feature end to end: `fingerprint_of` runs unconditionally regardless of rule count, at ~1.2 µs per
  query (§3, §7), because the audit record carries `sql_hash` on every terminal path.
- With rules present, `check` builds one `RecordBatch` from the borrowed attribution and evaluates
  each rule's `PhysicalExpr` against it, in order — ~3.4 µs at one rule, ~45 µs at the 100-rule cap
  (§3, §10).
- **Rules are kept in a stable order** — `created_at`, `rule_id` breaking ties — so the first match
  is the oldest matching rule, on every replica. Two replicas should name the same rule for the same
  query, since the rule id reaches the caller's error message, the `warn!` line, the audit record,
  and the per-rule metric. The oldest matching rule wins, which is also the one an operator is most
  likely to have forgotten about. Stable ordering is what this buys; it holds for any deterministic
  expression, and an admin who deliberately writes a non-deterministic one (§3, Grammar) gives it up
  knowingly rather than being prevented.
- `refresh` runs every `MICROMEGAS_QUERY_DENY_REFRESH_SECONDS` (default 10): it flushes each rule's
  `last_hit` (`UPDATE query_deny_list SET last_hit_at = greatest(coalesce(last_hit_at, $1), $1)
  WHERE rule_id = $2`, skipping rules not hit since the last flush), then reloads the table and
  recompiles each `match_expr`. Batching keeps a denied 4-QPS offender at one write per tick
  instead of one per rejection, and `last_hit_at` is therefore accurate to within one tick — which
  is all "is this rule still firing?" needs. The denial *rate* comes from the `query_denied` metric
  (§6), not from this table.
- **A rule that fails to compile is skipped, never fatal.** It is dropped from the snapshot with a
  `warn!` naming the rule id and the compile error, plus
  `imetric!("query_deny_compile_error_count", ...)`. This is what makes it safe to extend the match
  context later: an older replica that cannot compile a newer rule declines to enforce it rather
  than denying everything or crashing. It also absorbs the one real cost of borrowing DataFusion's
  parser — **the stored rule format is coupled to DataFusion's SQL dialect**, so an upgrade could in
  principle change how an existing `match_expr` parses. A rule that stops compiling stops being
  enforced, loudly, instead of silently changing meaning; the `query_deny_compile_error_count`
  metric is what an upgrade should be watched on.
- **Fail-open by design.** A failed refresh keeps the previous snapshot and emits
  `imetric!("query_deny_refresh_error_count", ...)` + a `warn!`. A failed *initial* load starts with
  an empty snapshot. The deny list is an availability valve, not a security control — failing closed
  would deny every query on a DB blip.
- The refresh task is spawned only by the FlightSQL server builder
  (`flight_sql_server.rs::build_and_serve`, which the monolith also uses). Other `LakehouseContext`
  holders (maintenance daemon, tests) keep an empty snapshot and never deny anything.
- `insert`/`delete` refresh the local snapshot synchronously before returning, so the admin who
  created a rule sees it in their own `list_query_denials()` immediately; other replicas pick it up
  within one tick.

### 5. The check in `execute_query`

Inserted immediately after `QueryAuditState` is constructed (`flight_sql_service_impl.rs:718`) and
before `scoped_runtime`/`caller_context`/`make_session_context`:

```rust
let denied = self.lakehouse.query_denials().check(&QueryAttribution { .. });
if let Some(rule) = denied
    && !skip_for_admin_recovery(&sql, caller_is_admin, self.admin_principal_possible)
{
    let status = Status::resource_exhausted(format!(
        "query denied by rule {} (reason: {}); ask an admin to lift it with \
         remove_query_denial('{}'); query_id={query_id}",
        rule.row.rule_id, rule.row.reason, rule.row.rule_id));
    warn!(
        "query denied rule_id={} reason={:?} sql_hash={sql_hash} user={} email={} \
         client={client_type} entrypoint={client_entrypoint} client_ip={client_ip} \
         query_id={query_id}",
        rule.row.rule_id, rule.row.reason, attr.user_id, attr.user_email);
    imetric!("query_denied", "count", rule_tags(&rule.row.rule_id), 1_u64);
    rule.record_hit();
    return Err(audit_state.fail_with_class(status, "denied"));
}
```

- **Status code**: `ResourceExhausted`. It is the only existing code whose `error_class` bucket
  (`"resource"`) already means "the service refused to spend resources on this", and it keeps the
  rejection out of the `query_failed`/`error!` internal-error path. The message is the
  distinguishing part: it names the rule id and the reason, and — since a rule has no expiry to
  wait out — tells the caller exactly what an admin has to run to lift it.
- **Warning log (§6)**: every denial emits a `warn!` line, so a denied query is visible on any
  dashboard already watching warning-level logs — not only to someone who thought to query the
  audit target. This is the one new log point on the deny path; without it the rejection would be
  silent at log level, since the deny site builds its `Status` directly and never passes through
  `error_or_warn_log` (which only ever sees a `DataFusionError`).
- **Audit record**: emitted with `status: "error"` and a dedicated `error_class: "denied"` (a
  fourth value alongside `user`/`resource`/`internal`), via a new
  `QueryAuditState::fail_with_class(status, class)` — `fail()` becomes a thin wrapper that derives
  the class from the code. Denied traffic stays fully visible in the audit log, which is how an
  operator confirms the offender actually backed off.
- **Anti-jam escape hatch.** A rule keyed on identity alone (e.g. `client_ip`) could match the
  admin's own recovery query and lock the valve shut — and with no expiry, nothing lifts it on its
  own. So the check is skipped when the caller can reach the admin functions at all —
  `is_admin || !admin_principal_possible`, the same predicate `register_lakehouse_functions` gates
  `deny_queries`/`remove_query_denial` on (Current State, "Admin gating") — **and** the statement
  mentions one of `deny_queries` / `remove_query_denial` / `list_query_denials`: three `contains`
  checks over the lowercased SQL text, nothing more. No call-position or token analysis: the let-chain
  above short-circuits, so this only ever runs for a query that is *already* being denied, and the
  gate it sits behind means any caller it opens for could call those three functions directly anyway.
  A caller who evaded a rule by mentioning `remove_query_denial` in a comment could equally just call
  it and delete the rule; nothing that was actually enforced is lost by matching the name loosely.
  Gating on `is_admin` alone would leave the hatch permanently shut in any
  deployment where `admin_principal_possible` is false (no admin principal configured: every API-key
  provider, or OIDC with an empty `admin_users` list) — `is_admin` is then always false while the
  mutating functions are registered for every caller, exactly the deployment this hatch most needs to
  work in. `skip_for_admin_recovery(sql, caller_is_admin, admin_principal_possible)`
  lives in `query_deny_list.rs` alongside the rest of the matching logic — it only needs
  the statement text and the two booleans, no `flight_sql_service_impl.rs`-private state — so it is a
  `pub` function in the analytics crate and its unit tests sit with the rest of that crate's tests.
  This is the primary recovery path, so it carries its own test, including the
  `admin_principal_possible == false` deployment shape.
- **Prepared statements are deliberately not checked.** `do_action_create_prepared_statement` is not
  a second insertion point, because nothing executes through it. It only plans — `ctx.sql()` to
  recover the result schema — and echoes the SQL back as the handle; `do_get_prepared_statement` is
  `api_entry_not_implemented!()`, so a prepared handle can never be executed. The Python client's
  `prepared_statement_stream` reflects this: it calls `query_stream(statement.query)`, which returns
  through `do_get_statement` → `execute_query`, where the check already sits. So there is no bypass
  to close and no scan cost to shed on that path — planning touches the catalog, not the data.
  Checking there would buy only earlier feedback during schema discovery, at the price of resolving
  attribution, `get_client_ip`, the `x-client-*` headers, and a fingerprint pass on an RPC that has
  none of them today and no audit record to write them to. One check site, on the path that actually
  spends the money, is the whole design. If `do_get_prepared_statement` is ever implemented, the
  check has to be added there — that is the trigger to revisit this, not the prepare RPC itself.

### 6. Making a denial visible

Three signals, deliberately at three different volumes:

**`warn!` per denial → `log_entries`.** Level `Warn` (3), under the
`micromegas::servers::flight_sql_service_impl` target, carrying the rule id, the reason, the
`sql_hash`, the caller (user/email/client/entrypoint/client_ip) and the `query_id`. Any panel that
already charts or lists warnings picks this up with no new wiring:

```sql
SELECT time, msg
FROM log_entries
WHERE level <= 3          -- Fatal, Error, Warn
  AND msg LIKE 'query denied%'
  AND time >= NOW() - INTERVAL '1 hour'
ORDER BY time DESC;
```

(A deployment watching `log_stats` instead sees the denials as a `Warn` rise on the
`micromegas::servers::flight_sql_service_impl` target — cheaper, but not denial-specific: that
target also carries the client-error warnings `error_or_warn_log` emits.)

**`imetric!("query_denied", "count", {rule_id}, 1)` → `measures`.** Tagged with the rule id (the
per-rule counter the issue asks for), which is why it is a tagged metric rather than an untagged
one: cardinality is bounded by `MICROMEGAS_QUERY_DENY_MAX_RULES` (100), well inside what a
`PropertySet` should carry. This is the rate signal a dashboard graphs:

```sql
SELECT date_bin(INTERVAL '1 minute', time) AS minute,
       property_get(properties, 'rule_id')  AS rule_id,
       sum(value)                           AS denied
FROM measures
WHERE name = 'query_denied'
  AND time >= NOW() - INTERVAL '6 hours'
GROUP BY minute, rule_id
ORDER BY minute;
```

**The audit record** (`error_class = 'denied'`) stays the full-detail row: SQL text, fingerprint,
full attribution. It is what an operator drills into once a panel says something is being denied.

**Volume.** A denied 4-QPS dashboard produces ~4 warnings/second for as long as its rule stands,
and rules no longer expire. That is the intended behavior — a standing denial *should* keep saying
so — and the metric and the audit record carry the exact rate regardless. If per-denial warning
volume ever proves too chatty in practice, the fix is the shape `db_api_key.rs::maybe_log_error`
already uses — a window derived from an existing interval with a fixed floor, so it cannot be
switched off — not a new env knob defaulting to disabled.

### 7. `CallerContext` gains the caller's identity

`deny_queries` must record `created_by`, and the UDTF only ever sees `CallerContext`. Add:

```rust
pub struct CallerContext {
    // ... existing fields ...
    pub identity: Option<String>,   // the caller's identity as recorded in `created_by` (§1);
                                    // None on internal/maintenance paths, and such a caller cannot
                                    // call `deny_queries`, which requires `Some` (§8)
}
```

One string, not a struct: `created_by` is the only consumer anywhere in this plan, so an email or a
service-account field would be written at every construction site and read nowhere. (The
`service_account` *match-context column* in §3 is unrelated and stays — it carries the authenticated
identity in the delegation case, where `user_id` is client-asserted.)

`FlightSqlServiceImpl::caller_context` takes the already-resolved attribution and populates it.
This is a **minor breaking change** to a `pub` Rust struct (CHANGELOG entry required); the compiler
enumerates every construction site, which is the intended failure mode.

`QueryAuditRecord` gains `pub sql_hash: String` — appended **last**, so existing JSON consumers are
unaffected. The doc comment on `error_class` is updated to enumerate `"denied"`.

### 8. Admin SQL surface

Registered inside the existing `caller.is_admin || !caller.admin_principal_possible` block in
`register_lakehouse_functions`.

**`list_query_denials()`** — UDTF, no args. Returns every rule currently in force:

| Column | Type | Notes |
|---|---|---|
| `rule_id` | Utf8 | |
| `created_at` | Timestamp(ns, UTC) | |
| `created_by` | Utf8 | |
| `reason` | Utf8 | |
| `match_expr` | Utf8 | the expression as written |
| `last_hit_at` | Timestamp(ns, UTC), nullable | NULL until the rule first fires; accurate to within one refresh tick |

**`deny_queries(match_expr, reason)`** — UDTF returning a single row (`rule_id`). The expression is
a boolean SQL predicate over the match context (§3); inner quotes are doubled, as anywhere else in
SQL:

```sql
SELECT * FROM deny_queries(
  'sql_hash = ''9f2c41ab73de0155'' AND entrypoint = ''grafana-alert''',
  'alert rule re-firing on failure; owner notified');
```

The table function's `call_with_args` is handed a `&dyn Session` (via `TableFunctionArgs::session`),
which has no SQL-expression parser — and rule compilation must produce the same result on every
replica regardless of whose session triggered it, so compilation goes through the bare
`SessionContext` the list holds for exactly this (§3, §4) either way, not the caller's session.
`call_with_args` itself runs, synchronously and all fail-loud with `plan_err!`: `compile_match_expr`
against that context (§3), reported with the offending token where the parser gives one; the
empty-`reason` check; the caller-identity check (`CallerContext::identity` must be `Some` — a `None`
identity, the maintenance/internal path, is rejected here, since `created_by` is `NOT NULL` and has
no sentinel to fall back on, §7); and the rule-count check against the current snapshot
(`MICROMEGAS_QUERY_DENY_MAX_RULES`). Only the DB write and the local snapshot refresh happen in the
async body behind `LogStreamTableProvider`/`TaskLogExecPlan`: `QueryDenyList::insert` takes the
already-compiled expression (plus the original `match_expr` text, stored verbatim) and cannot itself
fail on a bad expression.

**`remove_query_denial(rule_id)`** — scalar UDF returning a status string; deletes the row (returns
a clear "no such rule" message when it matched nothing). The audit log is the durable record of
what was denied and what it rejected, so the row itself does not need to survive its removal.

### 9. Web app — Admin → Query Deny List

The screen drives the **same SQL functions** through the existing `useStreamQuery` →
`/api/query-stream` path, against the data source the admin selects. No new REST routes and no
second copy of the rule store — which also means the screen manages the deny list of the
deployment it is pointed at, instead of whatever DB `analytics-web-srv` happens to hold (the
API-key pages' single-DB assumption would be wrong here).

Layout — a single rules table plus a create dialog (`tasks/query_deny_list_mockups/query-deny-list-screen.html`):

- **Rules table** — `SELECT * FROM list_query_denials()`: the match expression in monospace, reason,
  creator, created-at, **last hit** (relative — "4 s ago" reads as "still firing", "3
  weeks ago" as "probably removable"), **Remove** (via `ConfirmDialog`). Empty state points at the
  audit-log doc for finding an offender's fingerprint.
- **Deny a Query dialog** — an expression textarea plus the required reason, with insert-chips for
  the common predicates and a link to the match-context reference. A textarea rather than a field
  grid: the expression *is* the rule, and a grid can only ever express the AND-of-equalities
  subset. A compile error from the server is shown inline against the expression, not as a
  page-level banner.

The dialog issues `SELECT * FROM deny_queries('<expr>', '<reason>')`. **SQL literal escaping**: both
the expression and the reason are user-supplied and must go through a single `escapeSqlLiteral`
helper (`'` → `''`), applied at the one place that builds these statements — the same rule
`substitute_macros` already follows server-side. The textarea holds the expression in its natural,
single-quoted form (as the mockup shows — e.g. `sql_hash = '9f2c41ab73de0155' AND entrypoint =
'grafana-alert'`, and every insert-chip inserts a single-quoted fragment), so `escapeSqlLiteral`
doubles those quotes on the way out, exactly as it does for the reason field. The round trip is
worth a dedicated test.

Non-admins never reach the page (`AuthGuard requireAdmin`), and a non-admin who hand-typed the SQL
would get "function not found" from `flight-sql-srv` — the gate is server-side, the guard is UX.

`BLOCKED_FUNCTIONS` in `stream_query.rs` is deliberately **not** extended for the three new admin
functions themselves: they are admin-gated at `flight-sql-srv`, which is precisely what the web
screen needs to call. It is also left otherwise **untouched**. It substring-matches the entire
lowercased SQL text, so a deny expression that merely *mentions* one of the three
`BLOCKED_FUNCTIONS` names — e.g. `deny_queries('sql LIKE ''%retire_partitions%''', '…')` — is
rejected by the web dialog with a "destructive function" error, and that rule has to be authored
from `micromegas-query` or a notebook instead. Narrowing the guard to call position would mean
teaching it to skip comments as well, to keep `retire_partitions/*x*/()` from slipping through — a
bypass created by the narrowing, not by this feature. The existing guard is left as it is rather
than weakened as a side effect of shipping a deny list.

### 10. Configuration

| Env var | Default | Meaning |
|---|---|---|
| `MICROMEGAS_QUERY_DENY_REFRESH_SECONDS` | 10 | Snapshot refresh / `last_hit_at` flush interval; also the bound on cross-replica propagation |
| `MICROMEGAS_QUERY_DENY_MAX_RULES` | 100 | Cap on rules in force at once (bounds per-query cost) |

## Mockups

- `tasks/query_deny_list_mockups/query-deny-list-screen.html` — the rules table plus the "Deny a Query"
  dialog. The screen is purely a front end for the three SQL functions; the admin copies the
  fingerprint over from the audit log by hand.

An "incident console" variant was considered and dropped (a top-query-load panel driven by the
audit log, with a per-row *Deny…* button that prefills the dialog, plus a rejected-queries panel).
Triage does not belong on this screen: **finding the offender is a notebook's job** — a notebook
over the audit log listing the queries the service ran in the last few minutes, grouped by
`sql_hash` with their cost and attribution. That is an interactive, exploratory task where the
useful next step is usually another query, not a button; a fixed panel would answer one shape of
question and get in the way of the rest. That notebook is out of scope here — this plan owes it
only the `sql_hash` field it groups by and the documented query it builds on. The screen stays a
front end for the three SQL functions, and the admin pastes the fingerprint the notebook surfaced
into the dialog.

## Implementation Steps

### Phase 1 — Store and matching (analytics crate, no wiring)

1. `rust/analytics/src/lakehouse/migration.rs`: `upgrade_v8_to_v9` creating `query_deny_list`;
   `LATEST_LAKEHOUSE_SCHEMA_VERSION = 9`.
2. New `rust/analytics/src/lakehouse/query_deny_list.rs`: `fingerprint_of`, the match-context
   schema, `compile_match_expr` (parse → validate → `Arc<dyn PhysicalExpr>`), `QueryAttribution`,
   `QueryDenyRow`, `QueryDenyRule`, `DenySnapshot`, `skip_for_admin_recovery`, and `QueryDenyList`
   (`check` / `refresh` / `insert` / `delete` / `list` / `spawn_refresh_task`), env knobs. Register
   in `rust/analytics/src/lakehouse/mod.rs`. Add `sha2` to `analytics/Cargo.toml`.

   **Evaluation is DataFusion's `PhysicalExpr`** — `create_physical_expr` on the validated `Expr`,
   one row batch per query. It is a handful of lines, owns no SQL semantics, and is correct by
   construction (§3).
3. Unit tests for `fingerprint_of`, validation, and `skip_for_admin_recovery` (see Testing Strategy).

### Phase 2 — Wiring and enforcement

4. `lakehouse_context.rs`: construct and expose `query_denials()` (mirrors `audience_index()`).
5. `read_scope.rs`: add `CallerContext::identity: Option<String>`; fix every construction site
   the compiler flags.
6. `flight_sql_service_impl.rs`: call `fingerprint_of` once per query; add `sql_hash` to
   `QueryAuditState` and `QueryAuditRecord`; add `fail_with_class`; insert the deny-list check after
   `QueryAuditState` is built, with its `warn!` line and the rule-tagged `query_denied` metric (§6);
   populate `CallerContext::identity` in `caller_context`. `do_action_create_prepared_statement` is
   left untouched — it plans without executing and cannot be executed through, so it is not a check
   site (§5).
7. `flight_sql_server.rs`: spawn the refresh task with the existing shutdown fanout.

### Phase 3 — Admin SQL functions

8. `list_query_denials_table_function.rs` (pattern: `list_partitions_table_function.rs`).
9. `deny_queries_table_function.rs` — validates, inserts, refreshes the local snapshot, returns
   one row.
10. `remove_query_denial_udf.rs` (pattern: `retire_partition_by_file_udf.rs`).
11. Register all three in `query.rs`'s admin block.

### Phase 4 — Web app screen

12. `analytics-web-app/src/lib/query-deny-list-api.ts` — SQL builders (`escapeSqlLiteral`, the
    three statements) and Arrow→row decoding.
13. `analytics-web-app/src/routes/QueryDenyListPage.tsx` — the page, reusing
    `PageLayout` (via the `@/components/layout` barrel) / `AuthGuard requireAdmin` / `ErrorBanner` /
    `ConfirmDialog` / `DataSourceField` (exported from `components/DataSourceSelector.tsx`). There is
    no shared `Button` component in this app — buttons are inline Tailwind, as in
    `components/ApiKeysAdminPage.tsx`.
14. Register the route (`/admin/query-deny-list`) in `router.tsx` and add the card to
    `AdminPage.tsx` (lucide `ShieldBan` or `Ban` icon).
15. Vitest coverage for the SQL builders (escaping in particular) and a page render test, matching
    `routes/__tests__/AnalyticsApiKeysPage.test.tsx`.

### Phase 5 — Docs and changelog

16. `mkdocs/docs/admin/functions-reference.md`: the three functions, with the incident runbook.
17. `mkdocs/docs/query-guide/query-audit-log.md`: document `sql_hash` and `error_class = "denied"`,
    plus the "find the offender, copy its fingerprint" query.
18. `mkdocs/docs/admin/flight-sql.md`: the two env knobs, propagation delay, fail-open behavior.
19. `mkdocs/docs/admin/web-app.md`: the new admin screen, including the note that a deny expression
    naming a `BLOCKED_FUNCTIONS` function must be authored outside the web app (§9).
20. `mkdocs/docs/query-guide/python-api.md`: update the exception-types table and the "tell them
    apart" guidance (§ Exception types) — a denial is also `ResourceExhausted` /
    `pyarrow.lib.ArrowInvalid` with the same message prefix as a resource-budget failure, so both
    existing discriminators (message prefix, `error_class: "resource"`) stop being sufficient; add
    the deny-list row and point readers at `error_class: "denied"`.
21. `CHANGELOG.md`: feature entry + **Minor breaking change** clause for `CallerContext`.

## Files to Modify

**Create**

- `rust/analytics/src/lakehouse/query_deny_list.rs`
- `rust/analytics/src/lakehouse/list_query_denials_table_function.rs`
- `rust/analytics/src/lakehouse/deny_queries_table_function.rs`
- `rust/analytics/src/lakehouse/remove_query_denial_udf.rs`
- `rust/analytics/tests/query_deny_list_tests.rs` (unit)
- `rust/analytics/tests/query_deny_list_db_test.rs` (DB-backed, `#[ignore]`d)
- `analytics-web-app/src/lib/query-deny-list-api.ts`
- `analytics-web-app/src/routes/QueryDenyListPage.tsx`
- `analytics-web-app/src/lib/__tests__/query-deny-list-api.test.ts`
- `analytics-web-app/src/routes/__tests__/QueryDenyListPage.test.tsx`
- `python/micromegas/tests/test_query_deny_list.py`

**Modify**

- `rust/analytics/src/lakehouse/migration.rs`, `mod.rs`, `query.rs`, `read_scope.rs`,
  `lakehouse_context.rs`
- `rust/analytics/Cargo.toml` (`sha2.workspace = true`)
- `rust/public/src/servers/flight_sql_service_impl.rs`, `query_audit.rs`, `flight_sql_server.rs`
- `analytics-web-app/src/router.tsx`, `src/routes/AdminPage.tsx`
- `mkdocs/docs/admin/functions-reference.md`, `admin/flight-sql.md`, `admin/web-app.md`,
  `query-guide/query-audit-log.md`, `query-guide/python-api.md`
- `CHANGELOG.md`

## Trade-offs

**SQL functions as the single admin surface, with the web screen driving them.** The alternative —
REST routes on `analytics-web-srv` hitting Postgres directly, mirroring `analytics_keys.rs` — would
duplicate the rule store, its validation, and its error shape, and would target whichever DB
`analytics-web-srv` is configured with rather than the data source the admin selected. The SQL path
also gives CLI and notebook admins the same capability for free.

**Rules stored in Postgres and polled, not pushed.** Postgres is already the coordination point for
every replica and needs no new infrastructure. The cost is up to one refresh interval (10 s) of
propagation delay — irrelevant against an incident measured in minutes, and the inserting replica
applies its own rule immediately.

**`ResourceExhausted` rather than a new code or `PermissionDenied`.** `PermissionDenied` lands in
the `"internal"` `error_class` bucket, which would fire `query_failed` and `error!` logs for what is
a deliberate, expected rejection. `ResourceExhausted` already means "refused to spend resources
here"; the rule id, the reason, and the command that lifts it carry the distinguishing detail.

**One expression column, not a column per matcher.** A fixed matcher schema fossilizes the matching
language: every new attribute is a migration, and AND-of-equalities is the only combinator it can
ever express. A text column plus a compiler moves that evolution out of the schema entirely. The
cost is that the stored form is no longer directly queryable in SQL (`WHERE match_user = 'bob'` over
the rules table is gone) — which the rules table's size, and the fact that nobody queries it
programmatically, makes a non-issue.

**DataFusion parses *and* evaluates.** This check is a tax on every query for as long as a rule
stands, so it was measured (§3): 3.4 µs at one rule, ~45 µs at the 100-rule cap, most of the
single-rule figure being the one-row batch rather than the predicate. An evaluator of our own would
be two orders of magnitude faster, and prototypes confirmed it — but it would buy microseconds
inside a phase already measured in milliseconds, in exchange for owning Kleene logic, `LIKE`
lowering, and a differential test to keep them honest. The cheap thing was the correct thing:
DataFusion evaluates, `check` returns immediately when no rule stands, and a compiled evaluator
stays available behind an unchanged signature if a profile ever asks for it.

**Any boolean shape is accepted.** An earlier draft required every rule to carry a top-level
equality "anchor" so it could be pruned by a hash probe, which would have rejected
`sql LIKE '%thread_spans%' OR client_ip = '…'` — a legitimate and powerful incident rule that cannot
be anchored at all, since a disjunction has no conjunct to anchor on. Nothing about the evaluation
cost justifies constraining what an admin may write. The "must not match everything" guard is a
separate, semantic rule: an expression referencing no column is rejected.

**Regex is in, and the earlier ReDoS objection was wrong.** An earlier version of this plan excluded
regex because "caller-influenced regexes on a hot path invite ReDoS". That reasoning applies to
backtracking engines; the `regex` crate behind DataFusion's `regexp_like` does not backtrack and
guarantees linear time. Regex costs nothing to allow, so it is allowed.

**Fail-open on store errors.** A deny list that fails closed turns a Postgres blip into a total
outage — strictly worse than the problem it exists to solve. This is an availability valve, not an
authorization control; the authorization controls (`ReadScope`, `AudienceGuard`, `OwnershipRewrite`)
fail closed and are unaffected.

**No expiry: rules stand until removed.** The alternative — a mandatory, capped TTL — bounds the
damage of a forgotten rule automatically, but it also means a rule can silently lapse mid-incident
and let the offender back in while nobody is looking, and it forces an operator to guess a duration
up front. Standing rules keep the state of the world explicit: what is denied is exactly what
`list_query_denials()` shows. The cost is that a forgotten rule stays forgotten, which is mitigated
by three things — the mandatory `reason` and recorded `created_by`, the rejection message telling
the caller precisely what to ask for, and `last_hit_at`, which separates a rule still rejecting
traffic from one that has not fired in weeks and is safe to remove.

**Hard delete rather than soft delete.** `analytics_api_keys` keeps revoked rows with
`revoked_at`/`revoked_by`; the deny list does not, because the audit log already records every
denial the rule ever caused, which is the part worth keeping. If a "who removed this rule and when"
trail turns out to be wanted, a `removed_at`/`removed_by` pair plus a `WHERE removed_at IS NULL`
filter in the refresh query is an additive change.

**64-bit fingerprint.** Short enough to read off a log line and paste into a terminal; collisions
are astronomically unlikely and bounded in blast radius by the rule's other predicates.

## Documentation

- `mkdocs/docs/admin/functions-reference.md` — reference for the three functions, the **match
  context** columns and the **expression language** (§3) with worked examples, plus an "incident
  runbook" section: find the offender (a notebook over the audit log, out of scope here) → copy
  `sql_hash` → `deny_queries` → confirm rejections → `remove_query_denial` once the offending
  client is fixed.
- `mkdocs/docs/query-guide/query-audit-log.md` — the new `sql_hash` field, `error_class = "denied"`,
  and the top-offenders query an operator runs to find the fingerprint, documented here so it is
  usable directly and reusable by a triage notebook.
- `mkdocs/docs/admin/flight-sql.md` — env knobs, propagation delay, fail-open behavior, the admin
  escape hatch, and a **"Watching for denials"** section carrying both dashboard queries from §6
  (the warning-level `log_entries` panel and the per-rule `query_denied` rate panel) so an operator
  can paste them straight into a dashboard.
- `mkdocs/docs/admin/web-app.md` — the Admin → Query Deny List screen, noting that a deny
  expression that mentions one of the destructive-function names the web app blocks
  (`retire_partitions` and friends) is rejected by that guard and has to be created from
  `micromegas-query` or a notebook instead (§9).
- `mkdocs/docs/query-guide/python-api.md` — a denial is `ResourceExhausted` /
  `pyarrow.lib.ArrowInvalid` with the same message prefix as a resource-budget failure, so update
  the exception-types table with a deny-list row and tell readers to use `error_class: "denied"`
  (not the message prefix or `"resource"`) to distinguish the two.

## Testing Strategy

**Unit (`rust/analytics/tests/query_deny_list_tests.rs`, no DB)**

This is an external integration-test crate for `micromegas-analytics`, so it exercises the crate
through its `pub` surface: `fingerprint_of`, `compile_match_expr`, `check`, and
`skip_for_admin_recovery`.

- `fingerprint_of`: two dashboard refreshes differing only in timestamp/limit literals produce the
  same fingerprint; different column lists produce different ones; whitespace/comment/case changes
  are absorbed; unparseable SQL still yields a fingerprint.
- `compile_match_expr` rejects, each with a message naming the problem: a non-boolean result and an
  expression referencing no column (`true`, `1 = 1`) — its own two checks — plus unknown column,
  unknown function, and an aggregate, which DataFusion rejects for it at parse or plan time. The
  last three are tested to pin that the diagnostic reaches the admin, not that we produced it.
- **No coercion pass** (§3, step 3). Because `compile_match_expr` plans without `TypeCoercion`, the
  accepted subset is a property of DataFusion's physical planner, so it is pinned rather than
  assumed. Each of these compiles *and* evaluates against a batch from `QueryAttribution::to_batch`,
  since a `Utf8`-vs-`Utf8View`-style mismatch between `match_schema()` and `to_batch` would surface
  only at per-query evaluation on a replica that happens to hold a rule, never at compile time:
  `client IN ('grafana', 'python')` (in-list, the shape most likely to want a cast),
  `sql LIKE '%thread_spans%'` and its `ILIKE` form, `regexp_like(sql, '(?i)from\s+view_instance')`
  (a two-arg UDF signature matched with no coercion to help it), `notebook IS NOT NULL`, and a
  top-level `NOT`. Plus a direct assertion that `to_batch().schema()` equals `match_schema()` field
  for field — same names, same order, `DataType::Utf8` throughout — which is the invariant those
  evaluations depend on.
- **Type mismatches fail loudly, at compile time, not silently at match time**: `client = 42`
  (`Utf8` vs `Int64`) and `notebook = now()` (`Utf8` vs `Timestamp`) are both rejected by
  `compile_match_expr` with a message naming the two types, and neither is accepted as a rule that
  would then never fire. The second of these is also what covers the dropped non-`Immutable` guard
  (§3, Grammar): `now()` cannot reach a `Utf8` column in the first place.
- `user_id = 'svc-acct'` compiles and matches — pinning the reason the identity column is named
  `user_id` rather than `user`: under `GenericDialect`, a bare `user` parses as the zero-arg
  function `user()`, not a column reference.
- `check` semantics over a rule set: NULL attributes do not match an equality (`notebook =
  'fleet-overview'` does not fire for a query that sent no notebook header), a top-level `OR` rule
  denies when either side matches, and with two matching rules the older one — by
  `(created_at, rule_id)` — is the one returned.
- Every example expression in the docs compiles and evaluates as documented.
- `skip_for_admin_recovery` (defined and tested here, in `query_deny_list.rs`, since it only takes
  the statement text and two booleans): an admin statement calling `remove_query_denial` is exempt;
  the same statement from a non-admin is not; with `admin_principal_possible == false` (no admin
  principal configured), a non-admin caller's `remove_query_denial` statement is exempt too,
  matching `register_lakehouse_functions`' gate.

**Integration (`rust/analytics/tests/query_deny_list_db_test.rs` — `#[ignore]`d `#[tokio::test]`
requiring a live `MICROMEGAS_SQL_CONNECTION_STRING`, `mod common;` for `db_fixtures`, same
convention as `ownership_rewrite_db_test.rs`)**

- Migration v8 → v9 applies cleanly on a pre-existing lakehouse schema.
- `insert` → `refresh` → `check` matches; `delete` → `refresh` → no longer matches.
- Hit flush: several `record_hit` calls then `refresh` leaves `last_hit_at` at the most recent of
  them; a rule not hit this tick is not written at all and keeps its earlier `last_hit_at`.
- Refresh failure keeps the previous snapshot (point the pool at a closed connection).
- A row whose `match_expr` does not compile (written directly with `INSERT`, simulating a newer
  version) is skipped with a warning while every other rule stays enforced.

**Rust service tests (`rust/public/tests/`)**

- `QueryAuditRecord` with `error_class: "denied"` and `sql_hash` serializes as expected
  (extend `query_audit_tests.rs`).
- The denial `warn!` line contains the rule id, `sql_hash`, and caller attribution — asserted
  against the formatted string the same way `build_log_line`'s content is asserted today, rather
  than by capturing log output.

**End-to-end (`python/micromegas/tests/test_query_deny_list.py`, against `local_test_env`)**

- `deny_queries` with a `sql_hash` predicate → the matching query fails with a `ResourceExhausted`
  naming the rule id → `remove_query_denial` → the query succeeds again.
- A non-matching query is unaffected while the rule is in force.
- Each denial lands one `Warn`-level `log_entries` row (`msg LIKE 'query denied%'`) and one
  `query_denied` measure tagged with the rule id — the two dashboard signals from §6, checked
  end-to-end rather than only at the call site. Both rows only appear after the telemetry sink
  flushes and the maintenance role materializes the corresponding view, so both assertions poll via
  `otlp_helpers.assert_eventually` (already used for this in the test suite) with a timeout
  comfortably above `MICROMEGAS_FLUSH_PERIOD` (5 s in `local_test_env`), not a fixed sleep.
- A no-column expression is rejected, as is a syntactically invalid one, each with a message that
  names the problem.
- `list_query_denials()` shows the rule while it stands and drops it after removal; `last_hit_at` is
  populated once a refresh tick has flushed.
- A rule matching *everything the test client sends* still leaves `remove_query_denial` callable —
  the escape hatch (§5), and the only recovery path now that rules do not expire.
- Note: `local_test_env` runs with auth disabled, so every caller is an admin there.

**Web app (Vitest)**

- SQL builders: a reason containing `'` is escaped exactly once, and a single-quoted expression
  (e.g. `sql_hash = '9f2c41ab73de0155'`, matching the textarea's natural form) survives
  escape → server-side literal decoding unchanged.
- Page: renders rules, opens the deny dialog, calls the right SQL on confirm, shows the error
  banner on a failed query.

## Open Questions

None outstanding.
