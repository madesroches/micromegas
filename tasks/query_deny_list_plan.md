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
`execute_query`. `do_action_create_prepared_statement` (line 1235) is a second, cheaper planning
path that builds a session context and plans without executing.

### Audit record

`QueryAuditRecord` (`rust/public/src/servers/query_audit.rs:79`) already carries every field a rule
would want to match on: `client_ip`, `client`, `agent`, `entrypoint`, `session`, `notebook`, `cell`,
`user`, `email`, `service_account_name`, and the raw `sql`. It is emitted as one JSON line under the
`flightsql_query_audit` target on every terminal path — success, failure, and abandoned-mid-drain
(`QueryAuditState::emit`, line 314; `fail`, line 369; `Drop`, line 406). It does **not** currently
carry a normalized-SQL fingerprint.

`error_class` is derived from the gRPC code by `error_class()` (line 116): `InvalidArgument` /
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
  `LATEST_LAKEHOUSE_SCHEMA_VERSION = 8`. `LakehouseContext::from_env`/`new` run `migrate_lakehouse`
  at startup (`lakehouse_context.rs:45,60`), so **`flight-sql-srv` itself migrates on boot**.
- `AudienceIndex` (`rust/analytics/src/lakehouse/audience_guard.rs:202`) is the precedent for a
  Postgres-backed, TTL-cached lookup owned by `LakehouseContext` and reached from `query.rs` via
  `lakehouse.audience_index()`.
- Row-returning UDTF over Postgres: `list_partitions_table_function.rs`. Mutating UDTF:
  `retire_partitions_table_function.rs` (streams a log via `LogStreamTableProvider`).
- `sqlparser` 0.62 (with the `visitor` feature) is reachable as `datafusion::sql::sqlparser`;
  `sha2` is a workspace dependency; `regex` is **not** a direct dependency anywhere.

### Web app

- `AdminPage.tsx` is a card grid of admin destinations, each `AuthGuard requireAdmin`.
- `ApiKeysAdminPage.tsx` is the closest full-page precedent (table + create dialog + `ConfirmDialog`
  + `ErrorBanner`), but it talks to `analytics-web-srv` REST routes backed by the telemetry DB.
- SQL-driven pages (`ProcessesPage.tsx`, `ProcessLogPage.tsx`, …) use `useStreamQuery` →
  `POST /api/stream-query` (`rust/analytics-web-srv/src/stream_query.rs`), which forwards the
  browser user's own bearer token to FlightSQL via `BearerFlightSQLClientFactory`. So an admin's
  web query resolves `is_admin: true` at `flight-sql-srv` and sees the admin functions.
- `stream_query.rs:89` holds `BLOCKED_FUNCTIONS`, a substring blocklist that refuses
  `retire_partitions*` in web queries.

## Design

### 1. Storage — `query_deny_list` table (lakehouse migration v9)

```sql
CREATE TABLE query_deny_list (
  rule_id             UUID PRIMARY KEY,
  created_at          TIMESTAMPTZ  NOT NULL,
  created_by          VARCHAR(255) NOT NULL,
  reason              TEXT         NOT NULL,
  -- matchers: every non-NULL column must match for the rule to fire (AND)
  match_user          VARCHAR(255),
  match_email         VARCHAR(255),
  match_service_account VARCHAR(255),
  match_client        VARCHAR(255),
  match_agent         VARCHAR(255),
  match_entrypoint    VARCHAR(255),
  match_session       VARCHAR(255),
  match_notebook      VARCHAR(255),
  match_cell          VARCHAR(255),
  match_client_ip     VARCHAR(255),
  match_sql_hash      VARCHAR(64),
  match_sql_contains  TEXT,
  hit_count           BIGINT       NOT NULL DEFAULT 0
);
```

Flat nullable columns rather than a JSONB blob: `list_query_denials()` then returns a flat, stable
Arrow schema (the SQL layer is the stable interface — a new matcher is added as a **last** column),
and validation happens at insert time in one place. The *input* encoding for `deny_queries` is
still JSON (§3), so adding a matcher never changes a function signature.

No expiry column: a rule is in force from insertion until `remove_query_denial` deletes it. The
table holds at most `MICROMEGAS_QUERY_DENY_MAX_RULES` rows (§9) and needs no index — every replica
reads all of it on each refresh tick.

`LATEST_LAKEHOUSE_SCHEMA_VERSION` 8 → 9, `upgrade_v8_to_v9`. No `SCHEMA_VERSION` (partition
file-schema) change: no partition content is affected.

### 2. Normalized SQL fingerprint

New in `rust/analytics/src/lakehouse/query_deny_list.rs`:

```rust
/// Literal-stripped fingerprint of a statement: the first 16 hex chars of the
/// SHA-256 of the normalized token stream.
pub fn sql_fingerprint(sql: &str) -> String;
```

Implementation: `sqlparser::tokenizer::Tokenizer` over `GenericDialect`, then

- every `Token::Number`, `SingleQuotedString`, `DoubleQuotedString`, `NationalStringLiteral`,
  `HexStringLiteral`, `EscapedStringLiteral` → `?`
- whitespace runs collapse to one space; comments dropped
- keywords/identifiers lowercased (identifiers are already case-insensitive in DataFusion unless
  quoted; quoted identifiers keep their case since they arrive as `DoubleQuotedString`… which is
  also a literal token — see Open Questions)
- tokens joined with a single space, then SHA-256, hex, truncated to 16 chars (64 bits)

Tokenization failure falls back to hashing the whitespace-collapsed raw text, so a fingerprint
always exists.

This is what makes the dashboard case work: consecutive refreshes differ only in their time-range
literals, so they collapse to one fingerprint. A 64-bit fingerprint's collision odds across a
realistic query population are negligible, and the consequence of a collision is bounded by the
rule's other matchers.

The fingerprint is computed once per query in `execute_query` and stored on `QueryAuditState`, so
it costs nothing extra to also emit it in the audit record (§6) — which is how an operator gets the
value to paste into `deny_queries`.

### 3. Rule model, evaluation, and cache

```rust
pub struct QueryAttribution<'a> {      // borrowed view of what execute_query already resolved
    pub user: &'a str, pub email: &'a str,
    pub service_account: Option<&'a str>,
    pub client: &'a str, pub agent: &'a str, pub entrypoint: &'a str,
    pub session: Option<&'a str>, pub notebook: Option<&'a str>, pub cell: Option<&'a str>,
    pub client_ip: &'a str,
    pub sql: &'a str, pub sql_hash: &'a str,
}

pub struct QueryDenyRule {
    pub rule_id: Uuid, pub created_at: DateTime<Utc>, pub created_by: String,
    pub reason: String,
    pub matchers: QueryDenyMatchers,     // 12 Option<String> fields, mirrors the columns
    hits: AtomicU64,                      // in-process delta since the last flush
}

impl QueryDenyRule {
    /// Pure, offline-testable. Every `Some` matcher must match; a rule with no
    /// matcher at all can never be constructed (rejected at insert).
    pub fn matches(&self, q: &QueryAttribution<'_>) -> bool;
}
```

Matching semantics: exact, case-sensitive string equality for every field except
`match_sql_contains`, which is a case-insensitive substring test on the raw SQL. `Some(x)` against
an absent optional attribute (e.g. `match_notebook` when the query has no notebook) does not match.
A rule is in force for as long as its row exists — there is no time component to evaluate.

```rust
pub struct QueryDenyList {              // owned by LakehouseContext, like AudienceIndex
    pool: sqlx::Pool<sqlx::Postgres>,
    snapshot: std::sync::RwLock<Arc<Vec<Arc<QueryDenyRule>>>>,  // `arc-swap` is not a
                                   // workspace dep; a read lock held for one clone is enough
}

impl QueryDenyList {
    pub fn check(&self, q: &QueryAttribution<'_>) -> Option<Arc<QueryDenyRule>>;
    pub async fn refresh(&self) -> Result<()>;     // flush hit deltas, reload the rule set
    pub async fn insert(&self, ...) -> Result<QueryDenyRule>;
    pub async fn delete(&self, rule_id: Uuid) -> Result<bool>;
    pub async fn list(&self) -> Result<Vec<QueryDenyRule>>;
    pub fn spawn_refresh_task(self: Arc<Self>, shutdown: impl Future<Output = ()> + Send + 'static);
}
```

- `check` is a linear scan over a small `Vec` — no index. The number of rules is capped
  (`MICROMEGAS_QUERY_DENY_MAX_RULES`, default 100) at insert time, which bounds the per-query
  cost at a few hundred string comparisons on already-hot data.
- `refresh` runs every `MICROMEGAS_QUERY_DENY_REFRESH_SECONDS` (default 10): it first flushes each
  rule's accumulated `hits` delta (`UPDATE query_deny_list SET hit_count = hit_count + $1 WHERE
  rule_id = $2`, skipping zero deltas) and then reloads the whole table into a fresh snapshot.
  Batching the counter this way keeps a denied 4-QPS offender at one write per tick instead of one
  per rejection.
- **Fail-open by design.** A failed refresh keeps the previous snapshot and emits
  `imetric!("query_deny_refresh_error_count", ...)` + a `warn!`. A failed *initial* load starts
  with an empty snapshot. The deny list is an availability valve, not a security control — failing
  closed would deny every query on a DB blip.
- The refresh task is spawned only by the FlightSQL server builder
  (`flight_sql_server.rs::build_and_serve`, which the monolith also uses). Other `LakehouseContext`
  holders (maintenance daemon, tests) keep an empty snapshot and never deny anything.
- `insert`/`delete` refresh the local snapshot synchronously before returning, so the admin who
  created a rule sees it in their own `list_query_denials()` immediately; other replicas pick it up
  within one tick.

### 4. The check in `execute_query`

Inserted immediately after `QueryAuditState` is constructed (`flight_sql_service_impl.rs:718`) and
before `scoped_runtime`/`caller_context`/`make_session_context`:

```rust
let denied = self.lakehouse.query_denials().check(&QueryAttribution { .. });
if let Some(rule) = denied
    && !skip_for_admin_recovery(caller_is_admin, &sql_tokens)
{
    let status = Status::resource_exhausted(format!(
        "query denied by rule {} (reason: {}); ask an admin to lift it with \
         remove_query_denial('{}'); query_id={query_id}",
        rule.rule_id, rule.reason, rule.rule_id));
    warn!(
        "query denied rule_id={} reason={:?} sql_hash={sql_hash} user={} email={} \
         client={client_type} entrypoint={client_entrypoint} client_ip={client_ip} \
         query_id={query_id}",
        rule.rule_id, rule.reason, attr.user_id, attr.user_email);
    imetric!("query_denied", "count", rule_tags(&rule.rule_id), 1_u64);
    rule.record_hit();
    return Err(audit_state.fail_with_class(status, "denied"));
}
```

- **Status code**: `ResourceExhausted`. It is the only existing code whose `error_class` bucket
  (`"resource"`) already means "the service refused to spend resources on this", and it keeps the
  rejection out of the `query_failed`/`error!` internal-error path. The message is the
  distinguishing part: it names the rule id and the reason, and — since a rule has no expiry to
  wait out — tells the caller exactly what an admin has to run to lift it.
- **Warning log (§5)**: every denial emits a `warn!` line, so a denied query is visible on any
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
  own. So the check is skipped when the caller is admin **and** the statement references one of
  `deny_queries` / `remove_query_denial` / `list_query_denials` as an *identifier token* (checked
  over the token stream already produced for the fingerprint — not a substring match on the raw
  text). A non-admin cannot exploit this: those functions are not registered for them, and the skip
  requires `is_admin`. This is the primary recovery path, so it carries its own test.
- **Prepared statements**: the same check is applied at the top of
  `do_action_create_prepared_statement` (which plans, and is therefore worth protecting from a
  prepare loop). That path does not resolve attribution today; it will call
  `validate_and_resolve_user_attribution_grpc` for this. No audit record exists on that RPC, so the
  rejection is logged and counted but not audited.

### 5. Making a denial visible

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
so — but a deployment that finds a long-lived rule too chatty can set
`MICROMEGAS_QUERY_DENY_WARN_WINDOW_SECONDS` (default `0` = warn on every denial) to throttle the
line to at most once per rule per window, using the same checked-and-set `AtomicI64` pattern as
`db_api_key.rs::maybe_log_error`. The metric and the audit record are never throttled, so the exact
count survives regardless of what the log does.

### 6. `CallerContext` gains the caller's identity

`deny_queries` must record `created_by`, and the UDTF only ever sees `CallerContext`. Add:

```rust
pub struct CallerIdentity { pub user: String, pub email: String, pub service_account: Option<String> }

pub struct CallerContext {
    // ... existing fields ...
    pub identity: Option<CallerIdentity>,   // None on internal/maintenance paths
}
```

`FlightSqlServiceImpl::caller_context` takes the already-resolved attribution and populates it.
This is a **minor breaking change** to a `pub` Rust struct (CHANGELOG entry required); the compiler
enumerates every construction site, which is the intended failure mode.

`QueryAuditRecord` gains `pub sql_hash: String` — appended **last**, so existing JSON consumers are
unaffected. The doc comment on `error_class` is updated to enumerate `"denied"`.

### 7. Admin SQL surface

Registered inside the existing `caller.is_admin || !caller.admin_principal_possible` block in
`register_lakehouse_functions`.

**`list_query_denials()`** — UDTF, no args. Returns every rule currently in force:

| Column | Type | Notes |
|---|---|---|
| `rule_id` | Utf8 | |
| `created_at` | Timestamp(ns, UTC) | |
| `created_by` | Utf8 | |
| `reason` | Utf8 | |
| `hit_count` | Int64 | last flushed value |
| `match_user` … `match_sql_contains` | Utf8, nullable | one column per matcher, in table order |

**`deny_queries(matchers_json, reason)`** — UDTF returning a single row (`rule_id`). JSON keys are
the matcher names without the `match_` prefix:

```sql
SELECT * FROM deny_queries(
  '{"sql_hash": "9f2c41ab73de0155", "entrypoint": "grafana-alert"}',
  'alert rule re-firing on failure; owner notified');
```

Validation, all fail-loud with `plan_err!`/a returned error:
unknown JSON key; non-string value; empty object or all-empty values (a rule that would match
everything); empty `reason`; rule count already at `MICROMEGAS_QUERY_DENY_MAX_RULES`.

**`remove_query_denial(rule_id)`** — scalar UDF returning a status string; deletes the row (returns
a clear "no such rule" message when it matched nothing). The audit log is the durable record of
what was denied and what it rejected, so the row itself does not need to survive its removal.

### 8. Web app — Admin → Query Deny List

The screen drives the **same SQL functions** through the existing `useStreamQuery` →
`/api/stream-query` path, against the data source the admin selects. No new REST routes and no
second copy of the rule store — which also means the screen manages the deny list of the
deployment it is pointed at, instead of whatever DB `analytics-web-srv` happens to hold (the
API-key pages' single-DB assumption would be wrong here).

Layout — a single rules table plus a create dialog (`tasks/query_deny_list_mockups/query-deny-list-screen.html`):

- **Rules table** — `SELECT * FROM list_query_denials()`: matcher chips, reason, creator, created-at,
  hit count, **Remove** (via `ConfirmDialog`). Empty state points at the audit-log doc for finding
  an offender's fingerprint.
- **Deny a Query dialog** — one field per matcher plus the required reason. The admin brings the
  `sql_hash` over from the query audit log by hand (`mkdocs/docs/query-guide/query-audit-log.md`
  gains the query that surfaces it; the page links to it).

The dialog composes the JSON matcher object client-side and issues
`SELECT * FROM deny_queries('<json>', '<reason>')`. **SQL literal escaping**: both the JSON blob and
the reason are user-supplied and must go through a single `escapeSqlLiteral` helper (`'` → `''`),
applied at the one place that builds these statements — the same rule `substitute_macros` already
follows server-side.

Non-admins never reach the page (`AuthGuard requireAdmin`), and a non-admin who hand-typed the SQL
would get "function not found" from `flight-sql-srv` — the gate is server-side, the guard is UX.

`BLOCKED_FUNCTIONS` in `stream_query.rs` is deliberately **not** extended: these three functions are
admin-gated at `flight-sql-srv` and are precisely what the web screen needs to call.

### 9. Configuration

| Env var | Default | Meaning |
|---|---|---|
| `MICROMEGAS_QUERY_DENY_REFRESH_SECONDS` | 10 | Snapshot refresh / hit-count flush interval; also the bound on cross-replica propagation |
| `MICROMEGAS_QUERY_DENY_MAX_RULES` | 100 | Cap on rules in force at once (bounds per-query cost) |
| `MICROMEGAS_QUERY_DENY_WARN_WINDOW_SECONDS` | 0 | `0` warns on every denial; `>0` throttles the `warn!` line to once per rule per window (metric/audit unaffected) |

## Mockups

- `tasks/query_deny_list_mockups/query-deny-list-screen.html` — the rules table plus the "Deny a Query"
  dialog. The screen is purely a front end for the three SQL functions; the admin copies the
  fingerprint over from the audit log by hand.

An "incident console" variant was considered and dropped (a top-query-load panel driven by the
audit log, with a per-row *Deny…* button that prefills the dialog, plus a rejected-queries panel).
It can be layered onto the same page later without reworking the table or the dialog — see the
Open Questions.

## Implementation Steps

### Phase 1 — Store and matching (analytics crate, no wiring)

1. `rust/analytics/src/lakehouse/migration.rs`: `upgrade_v8_to_v9` creating `query_deny_list`;
   `LATEST_LAKEHOUSE_SCHEMA_VERSION = 9`.
2. New `rust/analytics/src/lakehouse/query_deny_list.rs`: `sql_fingerprint`, `QueryDenyMatchers`
   (+ JSON parse/validate), `QueryDenyRule::matches`, `QueryAttribution`, `QueryDenyList`
   (`check` / `refresh` / `insert` / `delete` / `list` / `spawn_refresh_task`), env knobs.
   Register in `rust/analytics/src/lakehouse/mod.rs`. Add `sha2` to `analytics/Cargo.toml`.
3. Unit tests for `sql_fingerprint` and `matches` (see Testing Strategy).

### Phase 2 — Wiring and enforcement

4. `lakehouse_context.rs`: construct and expose `query_denials()` (mirrors `audience_index()`).
5. `read_scope.rs`: add `CallerIdentity` + `CallerContext::identity`; fix every construction site
   the compiler flags.
6. `flight_sql_service_impl.rs`: compute the fingerprint once; add `sql_hash` to `QueryAuditState`
   and `QueryAuditRecord`; add `fail_with_class`; insert the deny-list check after `QueryAuditState`
   is built, with its `warn!` line and the rule-tagged `query_denied` metric (§5); populate
   `CallerContext::identity` in `caller_context`; add the check + attribution resolution to
   `do_action_create_prepared_statement`.
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
    `PageLayout` / `AuthGuard requireAdmin` / `ErrorBanner` / `ConfirmDialog` / `Button` /
    `DataSourceField`.
14. Register the route (`/admin/query-deny-list`) in `router.tsx` and add the card to
    `AdminPage.tsx` (lucide `ShieldBan` or `Ban` icon).
15. Vitest coverage for the SQL builders (escaping in particular) and a page render test, matching
    `routes/__tests__/AnalyticsApiKeysPage.test.tsx`.

### Phase 5 — Docs and changelog

16. `mkdocs/docs/admin/functions-reference.md`: the three functions, with the incident runbook.
17. `mkdocs/docs/query-guide/query-audit-log.md`: document `sql_hash` and `error_class = "denied"`,
    plus the "find the offender, copy its fingerprint" query.
18. `mkdocs/docs/admin/flight-sql.md`: the two env knobs, propagation delay, fail-open behavior.
19. `mkdocs/docs/admin/web-app.md`: the new admin screen.
20. `CHANGELOG.md`: feature entry + **Minor breaking change** clause for `CallerContext`.

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
- `rust/analytics/Cargo.toml` (`sha2`)
- `rust/public/src/servers/flight_sql_service_impl.rs`, `query_audit.rs`, `flight_sql_server.rs`
- `analytics-web-app/src/router.tsx`, `src/routes/AdminPage.tsx`
- `mkdocs/docs/admin/functions-reference.md`, `admin/flight-sql.md`, `admin/web-app.md`,
  `query-guide/query-audit-log.md`
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

**Substring, not regex, for SQL text matching.** `regex` is not a workspace dependency, and running
caller-influenced regexes on the hot path of every query invites ReDoS. The normalized fingerprint
covers the case the issue actually cares about (repeated dashboard refreshes), and substring covers
the rest. Regex can be added later behind the same JSON key set without a signature change.

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
the caller precisely what to ask for, and `hit_count`, which makes a stale rule that is still
rejecting traffic visible on the screen.

**Hard delete rather than soft delete.** `analytics_api_keys` keeps revoked rows with
`revoked_at`/`revoked_by`; the deny list does not, because the audit log already records every
denial the rule ever caused, which is the part worth keeping. If a "who removed this rule and when"
trail turns out to be wanted, a `removed_at`/`removed_by` pair plus a `WHERE removed_at IS NULL`
filter in the refresh query is an additive change.

**64-bit fingerprint.** Short enough to read off a log line and paste into a terminal; collisions
are astronomically unlikely and bounded in blast radius by the rule's other matchers.

## Documentation

- `mkdocs/docs/admin/functions-reference.md` — reference for the three functions, plus an "incident
  runbook" section: find the offender in the audit log → copy `sql_hash` → `deny_queries` →
  confirm rejections → `remove_query_denial` once the offending client is fixed.
- `mkdocs/docs/query-guide/query-audit-log.md` — the new `sql_hash` field, `error_class = "denied"`,
  and the top-offenders query an operator runs to find the fingerprint.
- `mkdocs/docs/admin/flight-sql.md` — env knobs, propagation delay, fail-open behavior, the admin
  escape hatch, and a **"Watching for denials"** section carrying both dashboard queries from §5
  (the warning-level `log_entries` panel and the per-rule `query_denied` rate panel) so an operator
  can paste them straight into a dashboard.
- `mkdocs/docs/admin/web-app.md` — the Admin → Query Deny List screen.

## Testing Strategy

**Unit (`rust/analytics/tests/query_deny_list_tests.rs`, no DB)**

- `sql_fingerprint`: two dashboard refreshes differing only in timestamp/limit literals produce the
  same fingerprint; different column lists produce different ones; whitespace/comment/case changes
  are absorbed; unparseable SQL still yields a fingerprint.
- `QueryDenyRule::matches`: AND semantics across combinations; `Some` matcher vs. absent optional
  attribute; case-insensitive `sql_contains`.
- Matcher JSON parsing: unknown key, non-string value, empty object, all-blank values, oversized
  reason — each rejected with a distinct message.
- `skip_for_admin_recovery`: an admin statement calling `remove_query_denial` is exempt; the same
  statement from a non-admin is not; a non-admin query that merely aliases a column
  `remove_query_denial` is not exempt.

**Integration (`rust/analytics/tests/query_deny_list_db_test.rs` — `#[ignore]`d `#[tokio::test]`
requiring a live `MICROMEGAS_SQL_CONNECTION_STRING`, `mod common;` for `db_fixtures`, same
convention as `ownership_rewrite_db_test.rs`)**

- Migration v8 → v9 applies cleanly on a pre-existing lakehouse schema.
- `insert` → `refresh` → `check` matches; `delete` → `refresh` → no longer matches.
- Hit-count flush: N `record_hit` calls then `refresh` leaves `hit_count = N` in Postgres.
- Refresh failure keeps the previous snapshot (point the pool at a closed connection).

**Rust service tests (`rust/public/tests/`)**

- `QueryAuditRecord` with `error_class: "denied"` and `sql_hash` serializes as expected
  (extend `query_audit_tests.rs`).
- The denial `warn!` line contains the rule id, `sql_hash`, and caller attribution — asserted
  against the formatted string the same way `build_log_line`'s content is asserted today, rather
  than by capturing log output.

**End-to-end (`python/micromegas/tests/test_query_deny_list.py`, against `local_test_env`)**

- `deny_queries` with a `sql_hash` matcher → the matching query fails with a `ResourceExhausted`
  naming the rule id → `remove_query_denial` → the query succeeds again.
- A non-matching query is unaffected while the rule is in force.
- Each denial lands one `Warn`-level `log_entries` row (`msg LIKE 'query denied%'`) and one
  `query_denied` measure tagged with the rule id — the two dashboard signals from §5, checked
  end-to-end rather than only at the call site.
- An empty-matcher rule is rejected.
- `list_query_denials()` shows the rule while it stands and drops it after removal; `hit_count`
  reflects the rejections once a refresh tick has flushed.
- A rule matching *everything the test client sends* still leaves `remove_query_denial` callable —
  the escape hatch (§4), and the only recovery path now that rules do not expire.
- Note: `local_test_env` runs with auth disabled, so every caller is an admin there.

**Web app (Vitest)**

- SQL builders: a reason containing `'` is escaped exactly once; the matcher JSON round-trips.
- Page: renders rules, opens the deny dialog, calls the right SQL on confirm, shows the error
  banner on a failed query.

## Open Questions

1. **Quoted identifiers in the fingerprint.** `sqlparser` tokenizes `"My Column"` as a
   `DoubleQuotedString`, indistinguishable from a string literal at the token level. Replacing it
   with `?` would collapse queries that differ only in a quoted column name into one fingerprint.
   Proposed: treat `DoubleQuotedString` as an identifier (keep it verbatim) and only strip
   single-quoted/number/hex/national literals. Worth confirming against real queries before
   implementing.
2. **Where should the "find the offender" step live?** This plan leaves it in the audit-log docs:
   the admin runs the top-offenders query themselves and pastes the fingerprint into the dialog.
   The mocked-up incident-console variant folded that into the same screen, and a standalone
   "Query Load" admin screen would serve it outside an incident too ("what is the service spending
   its time on?"). Either can be built later against the same `sql_hash` field; worth deciding
   before the screen's layout hardens.
3. **`match_email` case sensitivity.** Exact match is proposed; if the deployment's OIDC provider
   ever varies email case, a case-insensitive compare for `email`/`user` specifically may be worth
   it. Deferred until observed.
