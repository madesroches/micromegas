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
"function not found".

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
  per-request `is_admin`). The rest are internal-only session contexts that never execute
  admin-gated SQL — the maintenance daemon calls the underlying Rust functions
  (`write_partition::retire_partitions`, etc.) directly rather than through registered SQL
  functions:
  - `rust/analytics/src/lakehouse/view.rs:109`, `merge.rs:101`, `sql_batch_view.rs:87,154`,
    `export_log_view.rs:118,171`, `batch_partition_merger.rs:133`, `metadata.rs:182,287` — internal
    materialization/lookup contexts, all hardcoding `NoOpSessionConfigurator`.
  - `parse_block_table_function.rs:81`, `process_spans_table_function.rs:254`,
    `perfetto_trace_execution_plan.rs:232` — UDTF-internal contexts recursively built to run an
    inner query; reachable from user queries but only ever issue read-only SQL.
  - `rust/analytics/tests/thread_spans_ordering_db_test.rs:294` — test helper.
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
  functions it does cover.

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
    metadata
        .get("x-auth-is-admin")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<bool>().ok())
        .unwrap_or(false)
}
```

Fails closed: missing header, unparseable value, or unauthenticated request (no `AuthService`
configured) all resolve to `false`.

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

### 5. Pass `is_admin: false` at every internal call site

All ~11 other `make_session_context` call sites (listed in Current State) get a literal
`is_admin: false` — they never run SQL that references the mutating functions by name (the
maintenance daemon calls `write_partition::retire_partitions` etc. directly as Rust functions,
never through a registered UDTF), so this is a no-op change in behavior for them, and keeps the
mutating set out of contexts that have no business granting it.

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
6. Update all other `make_session_context` callers (internal engine code + the one test file) to
   pass `is_admin: false`, and update the direct `query()` call sites in
   `rust/analytics/tests/sql_view_test.rs` (lines 419, 444) and
   `rust/analytics/tests/histo_view_test.rs` (line 197) to pass `is_admin: false`.
7. **`mkdocs/docs/admin/functions-reference.md`**: note the new admin requirement on
   `retire_partitions`, `regenerate_partitions`, `retire_partition_by_metadata`, and
   `retire_partition_by_file` (the fifth mutating function, `materialize_partitions`, isn't
   documented on this page).
8. `cargo fmt` and `cargo clippy --workspace -- -D warnings` from `rust/`.

## Files to Modify

- `rust/auth/src/tower.rs`
- `rust/auth/src/axum.rs`
- `rust/public/src/servers/firehose_common.rs`
- `rust/auth/src/user_attribution.rs`
- `rust/analytics/src/lakehouse/query.rs`
- `rust/public/src/servers/flight_sql_service_impl.rs`
- `rust/analytics/src/lakehouse/view.rs`, `merge.rs`, `sql_batch_view.rs`, `export_log_view.rs`,
  `batch_partition_merger.rs`, `metadata.rs`, `parse_block_table_function.rs`,
  `process_spans_table_function.rs`, `perfetto_trace_execution_plan.rs` (call-site signature
  updates only — pass `is_admin: false`)
- `rust/analytics/tests/thread_spans_ordering_db_test.rs` (call-site signature update)
- `rust/analytics/tests/sql_view_test.rs`, `histo_view_test.rs` (direct `query()` call-site
  signature updates)
- `mkdocs/docs/admin/functions-reference.md` (document the new admin requirement)

## Trade-offs

- **Header round-trip vs. `Request` extensions.** The codebase already threads identity across
  the tower boundary via re-signed gRPC metadata headers rather than tonic `Request` extensions
  (extensions don't survive the `FlightSqlService` trait's own request handling in this setup).
  Adding `x-auth-is-admin` follows that existing, established pattern instead of introducing a
  second identity-propagation mechanism.
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

## Testing Strategy

- **`rust/auth/tests/tower_tests.rs`**: extend the existing `AuthService` tests (or add a new
  one) to assert `x-auth-is-admin` is set correctly from an `AuthContext` with `is_admin: true`
  and `is_admin: false`, and that a client-supplied `x-auth-is-admin: true` header is stripped
  before an unauthenticated/non-admin `AuthContext` reaches the inner service.
- **`rust/auth/tests/user_attribution_tests.rs`**: add unit tests for the new `is_admin(metadata)`
  helper — present/absent header, `"true"`/`"false"`/garbage values, case sensitivity if any.
- **`rust/analytics` tests** (new or extended, near `sql_view_test.rs`/`histo_view_test.rs`):
  build a session context with `make_session_context(..., is_admin: false)` and assert
  `ctx.sql("SELECT * FROM retire_partitions()")` (and the other four) fails with a
  function-not-found-style error; assert it succeeds (well-formed logical plan) with
  `is_admin: true`. Also assert non-mutating functions (`list_partitions`, `view_instance`, etc.)
  are unaffected by the flag either way.
- **Integration**: a FlightSQL end-to-end test (or extension of existing FlightSQL integration
  coverage) hitting `execute_query` with an API-key-authenticated client (always `is_admin:
  false`) confirms `retire_partitions()` is rejected, and with an admin-flagged OIDC token (test
  provider / `MICROMEGAS_ADMINS` entry) confirms it's accepted (up to actually executing —
  execution correctness is already covered elsewhere).
- **`do_action_create_prepared_statement`**: a regression test confirming it no longer ignores
  the request's metadata — e.g. that preparing `SELECT * FROM retire_partitions()` as a
  non-admin fails the same way `execute_query` does.
- Run `cargo test` from `rust/` for the affected crates (`auth`, `analytics`, `public`) plus the
  full `cargo clippy --workspace -- -D warnings`.

## Open Questions

- Should `retire_partition_by_file`/`retire_partition_by_metadata`'s existing
  `analytics-web-srv` substring blocklist (`stream_query.rs::BLOCKED_FUNCTIONS`) be extended to
  also cover `materialize_partitions`/`regenerate_partitions` while we're touching this area, or
  left alone as out-of-scope pre-existing tech debt superseded by this fix? Leaning toward
  leaving it alone (it's a different code path/service, and this plan's fix is the actual
  control), but flagging in case a quick fix is wanted alongside this change.
