# Gate Mutating Lakehouse UDFs on Admin Status (FlightSQL) Plan

## Overview

`register_lakehouse_functions` (`rust/analytics/src/lakehouse/query.rs`) registers every
lakehouse UDTF/UDF unconditionally on every `SessionContext`, including five mutating
functions: `retire_partitions`, `materialize_partitions`, `regenerate_partitions` (UDTFs) and
`retire_partition_by_file`, `retire_partition_by_metadata` (scalar UDFs). Any authenticated
FlightSQL caller — including a static API key, which is never admin — can invoke them today.
`AuthContext.is_admin` already exists and is enforced for `analytics-web-srv`'s admin HTTP
routes, but it is only *logged*, never checked, on the FlightSQL query path, and it doesn't
even cross the tower `AuthService` process boundary. This closes that hole: register the
mutating set only when the session's authenticated `is_admin` is true; everyone else gets
"function not found" — except when no `AuthService` is configured at all (`--disable-auth`,
the documented local-dev/monolith mode), which is treated as trusted/admin, matching the
existing `--disable-auth` convention already shipped in `web_server.rs` (see Design §2).

This is issue #1377, tracked independently of the broader data-isolation rollout (#1334) so it
can land now with today's `MICROMEGAS_ADMINS`-derived flag. It composes with — but does not
block on — #1369 (identity threading for `ReadScope`) and #1371 (UDTF/UDF read guards): per
`tasks/data_isolation/policy_based_data_isolation_plan.md` §4, the eventual registration
condition is *maintenance context ∨ admin ∨ `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS=true`*, but
the maintenance-context and opt-in knob arms belong to #1371. This plan implements only the
admin arm, since it is a standalone fix for a pre-isolation hole.

## Current State

- `AuthContext.is_admin` (`rust/auth/src/types.rs:32`) is populated at token validation
  (`rust/auth/src/oidc.rs:390,534,543`, matching subject/email against `MICROMEGAS_ADMINS`-style
  env allowlists) and is always `false` for API keys (`rust/auth/src/api_key.rs:124`).
- The gRPC path's `AuthService` tower layer (`rust/auth/src/tower.rs`) validates the request,
  then re-injects identity into gRPC metadata as headers so it survives the trip into the
  `FlightSqlServiceImpl` handlers: it strips any client-supplied `x-auth-subject`,
  `x-auth-email`, `x-auth-issuer`, `x-allow-delegation` (lines 107-110) and re-sets them from the
  validated `AuthContext` (lines 113-133). **`is_admin` is not one of these headers** — it never
  crosses this boundary.
- `rust/auth/src/user_attribution.rs::validate_and_resolve_user_attribution_grpc` reads
  `x-auth-subject`/`x-auth-email`/`x-allow-delegation` back out of metadata to build
  `UserAttribution` for audit logging. It has no equivalent for `is_admin`.
- `FlightSqlServiceImpl::execute_query` (`rust/public/src/servers/flight_sql_service_impl.rs:276`)
  has the gRPC `metadata: &MetadataMap` in scope and calls `make_session_context` at line 372.
- `FlightSqlServiceImpl::do_action_create_prepared_statement`
  (`rust/public/src/servers/flight_sql_service_impl.rs:835`) calls `make_session_context` at line
  842 to build a schema-only session for `ActionCreatePreparedStatementRequest`, but **discards
  its request** (`_request: Request<Action>`) — it has no identity resolution at all today. This
  is the same gap #1369 calls out ("the prepared-statement path... builds its session context
  with no identity resolution").
- `register_lakehouse_functions` (`rust/analytics/src/lakehouse/query.rs:96-171`) registers all
  UDTFs/UDFs unconditionally, including the five mutating ones (lines 120-123, 132-138, 139-145,
  167, 168-170). `make_session_context` (line 194) and `register_functions` (line 182) are the two
  wrappers that call it; neither takes an admin flag today.
- `make_session_context` has ~13 call sites. Two are the FlightSQL handlers above (need the real,
  per-request `is_admin`). The rest are internal-only session contexts; the maintenance daemon
  itself never constructs a FlightSQL client and so has no identity to gate — verified
  concretely: all five `CronTask`s in `rust/public/src/servers/maintenance.rs:296-381`
  (`every_day`/`every_hour`/`every_minute`/`every_second` materialization + `pg_stats`) reach
  mutation only through direct Rust calls, never through a registered SQL function —
  `write_partition.rs:367` calls `retire_partitions(&mut transaction, ...)` via `sqlx` inside
  `insert_partition_transaction`, and retention goes `delete.rs:166` →
  `retire_expired_partitions`. The remaining call sites split into two groups (see Design §5):
  - `rust/analytics/src/lakehouse/merge.rs:101`, `sql_batch_view.rs:87,154`,
    `export_log_view.rs:118,171`, `batch_partition_merger.rs:133` — internal materialization
    contexts that execute **caller-supplied** SQL (`count_src_query`/`extract_query`/
    `merge_partitions_query`) on `pub` types (`SqlBatchView`, `ExportLogView`,
    `BatchPartitionMerger`, `QueryMerger`), all hardcoding `NoOpSessionConfigurator`. Never
    reachable from a user session, but a downstream deployment could define a view whose
    src/transform/merge query names a mutating function — none of the three in-repo view
    constructors (`log_stats_view.rs`, `processes_view.rs`, `streams_view.rs`, wired by
    `default_view_factory` at `view_factory.rs:269-340`) do this, but `SqlBatchView`,
    `ExportLogView`, and `BatchPartitionMerger` are `pub` API with no in-repo constructor at all.
  - `rust/analytics/src/metadata.rs:182,282` — internal lookup contexts hardcoding
    `NoOpSessionConfigurator`, only ever issuing read-only SQL.
  - `parse_block_table_function.rs:81`, `process_spans_table_function.rs:254`,
    `perfetto_trace_execution_plan.rs:232` — UDTF-internal contexts recursively built to run an
    inner query; reachable from user queries but only ever issue read-only SQL.
  - `rust/analytics/tests/thread_spans_ordering_db_test.rs:294` — test helper. The same file also
    calls the free `query()` helper directly at lines 253, 263, 318 (via
    `use micromegas_analytics::lakehouse::query::query`) — also need the new `is_admin` argument.
  - `rust/analytics/tests/sql_view_test.rs:419,444` and
    `rust/analytics/tests/histo_view_test.rs:197` call the free `query()` helper directly (via
    `use micromegas_analytics::lakehouse::query::{query, ...}`), not `make_session_context` — also
    need the new `is_admin` argument.
- A second, unrelated, and incomplete defense already exists in `analytics-web-srv`'s own
  streaming query proxy: `contains_blocked_function`
  (`rust/analytics-web-srv/src/stream_query.rs:86-99`) substring-blocks `retire_partitions`,
  `retire_partition_by_metadata`, `retire_partition_by_file` in the raw SQL text before it even
  reaches FlightSQL. It doesn't cover `materialize_partitions`/`regenerate_partitions`, is
  trivially bypassed (comments, whitespace, aliasing), and only protects this one HTTP-to-
  FlightSQL proxy path, not FlightSQL clients directly. Out of scope to change, but worth noting:
  once the FlightSQL-level gate lands, this becomes redundant (harmless) defense-in-depth for the
  functions it does cover. `analytics-web-srv` forwards the *end user's own* OIDC token to
  FlightSQL, not a service credential — `rust/analytics-web-srv/src/auth/handlers.rs:504` inserts
  `AuthToken(id_token)` from the validated cookie/bearer token, and `stream_query.rs:244-248`
  hands that token to `BearerFlightSQLClientFactory`. So once the FlightSQL-level gate lands, an
  admin web-app user is correctly recognized as admin at the FlightSQL layer and legitimately
  retains `materialize_partitions`/`regenerate_partitions` access (neither is in today's
  blocklist). The blocklist should stay as-is, unextended: the FlightSQL registration gate is the
  actual control, and extending the substring blocklist to also cover these two functions would
  take that access away from admins the gate legitimately allows, while adding nothing for
  non-admins, who are already stopped by the registration gate.

## Design

### 1. Thread `is_admin` across the tower `AuthService` boundary

Add a fifth header, `x-auth-is-admin`, alongside the existing four in
`rust/auth/src/tower.rs::AuthService::call`: strip any client-supplied copy, then set it from
`auth_ctx.is_admin.to_string()` — the exact pattern already used for `x-allow-delegation`
(lines 107-110, 129-133).

For defense-in-depth, also add `x-auth-is-admin` to the client-header-stripping lists in
`rust/auth/src/axum.rs::auth_middleware` (lines 69-72) and
`rust/public/src/servers/firehose_common.rs::firehose_auth_middleware` (lines 102-105), even
though neither path currently reads or sets it — these are the general "never trust a
client-supplied auth-derived header" boundaries, and a future consumer must not be able to
inherit a spoofed value through them. (The HTTP path (`analytics-web-srv`) already gets
`is_admin` from the `AuthContext` in request extensions, not headers, so it needs no new
plumbing.)

### 2. Extract `is_admin` on the FlightSQL side

Add a small helper in `rust/auth/src/user_attribution.rs`, next to
`validate_and_resolve_user_attribution_grpc` — that module already owns "read auth headers back
out of metadata" (it does the same for `x-auth-subject`/`x-auth-email`/`x-allow-delegation`), so
`is_admin` belongs alongside it rather than inline in `flight_sql_service_impl.rs`:

```rust
pub fn is_admin(metadata: &MetadataMap) -> bool {
    match metadata.get("x-auth-is-admin") {
        // No `AuthService` configured (e.g. `--disable-auth`) never sets any `x-auth-*`
        // header — this is the only way the header can be absent, since `AuthService::call`
        // rejects the request with `Unauthenticated` before it reaches the inner service
        // when a provider *is* configured but validation fails. Treat this case as trusted
        // admin, matching the existing `--disable-auth` convention in
        // `analytics-web-srv/src/web_server.rs` (which injects `ValidatedUser { is_admin:
        // true, .. }` when auth is disabled).
        None => true,
        Some(v) => v
            .to_str()
            .ok()
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(false),
    }
}
```

Fails closed only for an authenticated-but-non-admin caller: an unparseable value resolves to
`false`. A missing header resolves to `true`, since (per `rust/auth/src/tower.rs`) the header is
only ever absent when no `AuthService` is configured at all — the same "no auth configured means
trusted" case `web_server.rs` already handles for its HTTP admin routes.

### 3. Add an `is_admin: bool` parameter to the registration/session-context functions

In `rust/analytics/src/lakehouse/query.rs`:
- `register_lakehouse_functions(ctx, lakehouse, part_provider, query_range, view_factory,
  is_admin: bool)` — wrap the five mutating registrations (lines 120-123, 132-138, 139-145, 167,
  168-170) in `if is_admin { ... }`.
- `register_functions(...)` and `make_session_context(...)` gain the same `is_admin: bool`
  parameter and pass it straight through.
- The free `query()` helper (line 239, test-only today) gains it too, for symmetry.

This is a plain parameter, not a `SessionConfigurator` hook — `SessionConfigurator` is for
strictly additive, post-registration customization (registering extra tables), and gating what
DataFusion already knows about a function name is a registration-time concern, not a
session-extension one.

### 4. Wire the real value in on the two user-facing call sites

- `FlightSqlServiceImpl::execute_query` (line 372): pass
  `micromegas_auth::user_attribution::is_admin(metadata)`.
- `FlightSqlServiceImpl::do_action_create_prepared_statement` (line 842): change
  `_request: Request<Action>` to `request: Request<Action>` and pass
  `micromegas_auth::user_attribution::is_admin(request.metadata())`. This also closes the
  "no identity resolution at all" gap on this path for the admin gate specifically (a full
  `ReadScope` fix for this path is #1369's job; this only needs `is_admin`).

### 5. Split internal call sites: `is_admin: true` where caller-supplied SQL is executed internally, `is_admin: false` elsewhere

The ~11 other `make_session_context` call sites (listed in Current State) are not uniform: four
of them build a session to execute **caller-supplied** SQL text (`count_src_query`/
`extract_query`/`merge_partitions_query` on the `pub` types `SqlBatchView`, `ExportLogView`,
`BatchPartitionMerger`, and `QueryMerger`), while the rest only ever issue hardcoded, read-only
SQL that this repo controls.

- Pass **`is_admin: true`** at the four internal-materialization sites that execute
  caller-supplied SQL: `merge.rs:101`, `sql_batch_view.rs:87,154`, `export_log_view.rs:118,171`,
  `batch_partition_merger.rs:133`. These contexts are never reachable from a user session (they
  only ever run inside the maintenance daemon's own materialization pipeline), so granting the
  mutating set there costs nothing security-wise, and it avoids a downstream-breakage class: a
  deployment defining a `SqlBatchView`/`ExportLogView`/custom merge query whose SQL happens to
  name a mutating function would otherwise silently start failing under a hardcoded `false` (none
  of the three in-repo view constructors do this today, but these three types are `pub` API with
  no in-repo constructor at all, so nothing in this repo exercises the risk either way).
- Keep **`is_admin: false`** at the genuinely user-reachable and internal-lookup sites: the
  UDTF-internal contexts recursively built to run an inner read-only query
  (`parse_block_table_function.rs:81`, `process_spans_table_function.rs:254`,
  `perfetto_trace_execution_plan.rs:232`), the internal lookup contexts in `metadata.rs:182,282`,
  and the test call site — none of these have any business granting the mutating set.

## Implementation Steps

1. **`rust/auth/src/tower.rs`**: add `x-auth-is-admin` to the strip list and the re-injected
   header set in `AuthService::call`.
2. **`rust/auth/src/axum.rs`** and **`rust/public/src/servers/firehose_common.rs`**: add
   `x-auth-is-admin` to their client-header strip lists.
3. **`rust/auth/src/user_attribution.rs`**: add `pub fn is_admin(metadata: &MetadataMap) -> bool`.
4. **`rust/analytics/src/lakehouse/query.rs`**: add `is_admin: bool` to
   `register_lakehouse_functions`, `register_functions`, `make_session_context`, `query`; gate
   the five mutating registrations.
5. **`rust/public/src/servers/flight_sql_service_impl.rs`**: thread `is_admin` through the two
   call sites (`execute_query`, `do_action_create_prepared_statement`), fixing the latter's unused
   `_request` parameter.
6. Update the remaining `make_session_context` callers: pass `is_admin: true` at the four
   internal-materialization sites that run caller-supplied SQL (`merge.rs:101`,
   `sql_batch_view.rs:87,154`, `export_log_view.rs:118,171`, `batch_partition_merger.rs:133`),
   and `is_admin: false` at the genuinely user-reachable/read-only sites
   (`parse_block_table_function.rs:81`, `process_spans_table_function.rs:254`,
   `perfetto_trace_execution_plan.rs:232`, `metadata.rs:182,282`) plus the test site
   (`thread_spans_ordering_db_test.rs:294`). Also update the direct `query()` call sites in
   `rust/analytics/tests/thread_spans_ordering_db_test.rs` (lines 253, 263, 318),
   `rust/analytics/tests/sql_view_test.rs` (lines 419, 444) and
   `rust/analytics/tests/histo_view_test.rs` (line 197) to pass `is_admin: false`.
7. **`mkdocs/docs/admin/functions-reference.md`**: note the new admin requirement on
   `retire_partitions`, `regenerate_partitions`, `retire_partition_by_metadata`, and
   `retire_partition_by_file` (the fifth mutating function, `materialize_partitions`, isn't
   documented on this page). **`mkdocs/docs/query-guide/functions-reference.md`**: this is the
   user-facing catalog of *all* Micromegas SQL extensions, and it documents all five mutating
   functions (lines 47, 53, 59, 71, 77) alongside `list_partitions()` (line 41) and
   `list_view_sets()` (line 65) — which remain callable by every caller — all marked with the
   same 🔧, with no legend anywhere in `mkdocs/` defining what 🔧 means. Add an admin-required
   note to the five mutating entries, and split the marker (e.g. keep 🔧 as a general
   "administrative-flavored function" marker, add a new 🔒 "requires admin" marker for the five
   gated entries, and add a short legend explaining both) so gated and non-gated functions are
   visually distinguishable. **`mkdocs/docs/admin/maintenance.md:165-168`**: this page also lists
   `materialize_partitions()`, `regenerate_partitions()`, `retire_partitions()`, and
   `retire_partition_by_metadata()` as the ad-hoc administration path — add the same
   admin-required note there. **`mkdocs/docs/query-guide/python-api.md`**: add the same "requires
   admin — see Admin SQL Functions" note to its `materialize_partitions()`,
   `regenerate_partitions()`, and `retire_partitions()` sections (around lines 474-528), since
   these are the worked Python client examples users read before calling the now-gated
   functions. **`python/micromegas/micromegas/flightsql/client.py`**: add the same "requires
   admin" note to the docstrings of `retire_partitions()`, `materialize_partitions()`, and
   `regenerate_partitions()` (around lines 607-733) — these are a separate hand-written source
   from `python-api.md`, not generated from it. **`python/micromegas/micromegas/admin.py`**: add
   a note to `retire_incompatible_partitions()`'s docstring (around line 87) that it now requires
   `is_admin`, since it internally issues raw SQL calling `retire_partition_by_metadata()`
   (around line 173) on the caller's behalf and would otherwise start failing for non-admin
   callers with no explanation.
8. `cargo fmt` and `cargo clippy --workspace -- -D warnings` from `rust/`.

## Files to Modify

- `rust/auth/src/tower.rs`
- `rust/auth/src/axum.rs`
- `rust/public/src/servers/firehose_common.rs`
- `rust/auth/src/user_attribution.rs`
- `rust/analytics/src/lakehouse/query.rs`
- `rust/public/src/servers/flight_sql_service_impl.rs`
- `rust/analytics/src/lakehouse/merge.rs`, `sql_batch_view.rs`, `export_log_view.rs`,
  `batch_partition_merger.rs` (call-site signature updates only — pass `is_admin: true`; these
  run caller-supplied SQL in internal-materialization contexts never reachable from a user
  session — see Design §5)
- `rust/analytics/src/lakehouse/parse_block_table_function.rs`,
  `process_spans_table_function.rs`, `perfetto_trace_execution_plan.rs` (call-site signature
  updates only — pass `is_admin: false`)
- `rust/analytics/src/metadata.rs` (call-site signature updates only — pass `is_admin: false`)
- `rust/analytics/tests/thread_spans_ordering_db_test.rs` (call-site signature update for
  `make_session_context`, plus the direct `query()` call sites at lines 253, 263, 318)
- `rust/analytics/tests/sql_view_test.rs` (direct `query()` call-site signature updates, plus the
  new `#[ignore]`d admin-gate regression test — see Testing Strategy), `histo_view_test.rs`
  (direct `query()` call-site signature updates)
- `mkdocs/docs/admin/functions-reference.md` (document the new admin requirement)
- `mkdocs/docs/query-guide/functions-reference.md` (document the new admin requirement on the
  five mutating entries and split the `🔧` legend so gated vs. non-gated functions are
  distinguishable)
- `mkdocs/docs/admin/maintenance.md` (note the admin requirement in the "Ad-hoc administration"
  section, lines 165-168)
- `mkdocs/docs/query-guide/python-api.md` (note the new admin requirement on the
  `materialize_partitions()`, `regenerate_partitions()`, and `retire_partitions()` client
  wrapper sections)
- `python/micromegas/micromegas/flightsql/client.py` (add "requires admin" notes to the
  `retire_partitions()`, `materialize_partitions()`, and `regenerate_partitions()` docstrings)
- `python/micromegas/micromegas/admin.py` (add a "requires admin" note to
  `retire_incompatible_partitions()`'s docstring)
- `rust/auth/tests/tower_tests.rs` (extend `AuthService` tests for `x-auth-is-admin`)
- `rust/auth/tests/user_attribution_tests.rs` (add unit tests for the new `is_admin(metadata)`
  helper)

## Trade-offs

- **Header round-trip vs. `Request` extensions.** The codebase already threads identity across
  the tower boundary via re-signed gRPC metadata headers (`x-auth-subject`/`x-auth-email`/
  `x-allow-delegation`); adding `x-auth-is-admin` follows that existing, established pattern
  instead of introducing a second identity-propagation mechanism.
  **Considered and rejected:** `request.extensions().get::<AuthContext>()` is in fact available
  at both call sites this plan targets — `AuthService::call` already does
  `parts.extensions.insert(auth_ctx)` (`rust/auth/src/tower.rs:135`); tonic copies
  `parts.extensions` into the gRPC `Request` (`tonic-0.14.6/src/request.rs:160-167`
  `from_http_parts`, invoked from `tonic-0.14.6/src/server/grpc.rs:388,416`); and
  `arrow-flight-58.3.0/src/sql/server.rs:681,707,880` passes that same, unmodified `Request` into
  `do_get_statement`, `do_get_fallback`, and `do_action_create_prepared_statement`. Reading
  `is_admin` from extensions would be unspoofable by construction (no header contract to
  strip/re-set) and would make the `axum.rs`/`firehose_common.rs` strip-list edits in Design §1
  unnecessary. The header approach was chosen anyway, purely for consistency with the pattern
  already in place — not because extensions don't work — so this is a stylistic/consistency
  trade-off, not a technical constraint.
- **No `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` opt-in here.** The data-isolation plan's eventual
  gate is *maintenance ∨ admin ∨ opt-in-knob*; this plan implements only the admin arm. Adding the
  opt-in knob now would require deciding its config plumbing ahead of #1371's broader
  `ReadScope`/grant-config work, for a deployment mode (non-admins doing maintenance work) that
  isn't the common case. Deferring it to #1371 keeps this fix small and matches the issue's own
  scoping ("The admin arm is tracked independently as issue #1377").
- **Behavior change, not a compatibility-preserving default.** Before this change, every
  authenticated caller (including API keys) can call the five mutating functions; after, only
  admins can. This is the intended effect (closing a real hole), not an oversight — non-admin
  callers relying on `retire_partitions`/`materialize_partitions`/etc. today will need
  `is_admin` or must wait for #1371's opt-in knob.
- **`is_admin` extraction helper location.** Placed in `rust/auth/src/user_attribution.rs`
  (alongside the existing header-reading logic) rather than inline in
  `flight_sql_service_impl.rs`, so `rust/auth` stays the single place that knows the
  `x-auth-*` header contract.
- **Breaking change to a published-crate public API.** `register_lakehouse_functions`,
  `register_functions`, `make_session_context`, and `query` (`rust/analytics/src/lakehouse/
  query.rs`) are all `pub` and re-exported by the published `micromegas` crate
  (`rust/public/src/lib.rs:152`, `pub mod analytics { pub use micromegas_analytics::*; }` under
  the `server` feature). Adding a required `is_admin: bool` parameter is a breaking signature
  change for any external caller of these four functions — call this out explicitly in the PR
  description / CHANGELOG entry, per the precedent in
  `tasks/completed/1037_graceful_shutdown_plan.md:140`.
- **Missing header means trusted, not denied.** This matches the existing `--disable-auth`
  convention already shipped in `web_server.rs` (which synthesizes `ValidatedUser { is_admin:
  true, .. }` when auth is disabled), and keeps the documented local-dev/monolith workflow
  (`--disable-auth`, as started by `local_test_env/ai_scripts/start_services.py`) working: without
  this carve-out, `is_admin(metadata)` would always return `false` under `--disable-auth` and every
  local invocation of the mutating functions via `micromegas-query`/the Python client would start
  failing with "function not found".

## Testing Strategy

- **`rust/auth/tests/tower_tests.rs`**: extend the existing `AuthService` tests (or add a new
  one) to assert `x-auth-is-admin` is set correctly from an `AuthContext` with `is_admin: true`
  and `is_admin: false`, and that a client-supplied `x-auth-is-admin: true` header is stripped
  before an unauthenticated/non-admin `AuthContext` reaches the inner service.
- **`rust/auth/tests/user_attribution_tests.rs`**: add unit tests for the new `is_admin(metadata)`
  helper — present/absent header, `"true"`/`"false"`/garbage values, case sensitivity if any, and
  the disabled-auth case (no `x-auth-is-admin` header at all, i.e. an empty `MetadataMap`) asserting
  `is_admin` returns `true`, mirroring `web_server.rs`'s existing `--disable-auth` behavior.
- **`rust/analytics/tests/sql_view_test.rs`** (extend the existing test file; this remains a
  `#[ignore]`d DB-and-object-store-backed test, same as its neighbors — see Current State, there
  is no DB-free way to build a `LakehouseContext`/`make_session_context` today, so `cargo test` in
  CI (`build/rust_ci.py:27`) won't run it, and the `rust/auth` unit tests above are what actually
  cover this change in CI; this test is a manual/local regression check, not a CI gate): build a
  session context with `make_session_context(..., is_admin: false)` and assert that well-formed
  calls to all five mutating functions fail to *plan*, asserting on the error *message*, not just
  that planning errored — for the three UDTFs, `ctx.sql("SELECT * FROM
  retire_partitions('log_entries', 'i', TIMESTAMP '2024-01-01T00:00:00Z', TIMESTAMP
  '2024-01-02T00:00:00Z')")`, the equivalent well-formed `materialize_partitions(...)` and
  `regenerate_partitions(...)` calls, expect an error whose message contains `"table function"`
  and `"not found"`; for the two scalar UDFs, `ctx.sql("SELECT
  retire_partition_by_file('s3://bucket/x/file.parquet')")` and the equivalent
  `retire_partition_by_metadata(...)` call, expect an error whose message contains `"Invalid
  function"`. With `is_admin: true`, assert the same five well-formed calls plan successfully
  (`ctx.sql(...).await.is_ok()`) — do not assert on execution, which needs a live lakehouse and is
  out of scope here. Also assert non-mutating functions (`list_partitions`, `view_instance`, etc.)
  are unaffected by the flag either way.
- **Integration**: no existing harness starts an in-process `FlightSqlServer` for tests —
  `rust/*/tests/` has no such fixture, and the only e2e coverage is the Python suite in
  `python/micromegas/tests/`, which runs against
  `local_test_env/ai_scripts/start_services.py`; that script passes `--disable-auth` to
  `flight-sql-srv` (line 192), so `is_admin` is always `true` there and the non-admin rejection
  path is untestable without new harness work (a test auth provider plus API-key/OIDC test
  clients), which is out of scope for this plan. Scoping down to what exists: coverage for this
  change is the tower-level `x-auth-is-admin` tests (`rust/auth/tests/tower_tests.rs`) plus the
  `rust/analytics/tests/sql_view_test.rs` registration-gate test above — together they cover
  header propagation end-to-end and the registration gate itself, just not a live FlightSQL
  round-trip. This is a known, accepted coverage gap, not an oversight.
- **`do_action_create_prepared_statement`**: for the same reason, this is covered at the unit
  level only — the `rust/auth` unit tests for `is_admin(metadata)` cover header parsing, and the
  `_request` → `request` signature change (so the handler stops discarding the request's
  metadata) is verified by code review, not a new integration test. A live "prepare as non-admin,
  expect rejection" test needs the same missing FlightSQL-server-with-auth harness as the
  Integration bullet above and is not added here.
- Run `cargo test` from `rust/` for the affected crates (`auth`, `analytics`, `public`) plus the
  full `cargo clippy --workspace -- -D warnings`.
