# Test Suite Audit (Pertinence & Efficiency) Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1219

## Overview

Issue #1219 asks for an audit of the Rust and Python test suites against five criteria: dead/obsolete
coverage, redundant/duplicate tests, slow tests, hardcoded-sleep/flaky patterns, and coverage gaps.
This plan documents what was found for each criterion (verified against the current code, not just
grep hits) and lists the small number of concrete, low-risk fixes that follow directly from those
findings. Most effort here is investigation; the fix set is intentionally narrow so the change stays
safe to review and merge as one PR.

## Current State

- CLAUDE.md (`/home/mad/micromegas/CLAUDE.md`) mandates that unit tests live under each crate's
  `tests/` folder rather than inline `#[cfg(test)]` modules in `src/`.
- A prior plan, `tasks/completed/unit_tests_in_memory_recording_plan.md`, already converted the
  `tracing`/`analytics` telemetry tests off `TelemetryGuardBuilder` onto `InMemorySink`, standardized
  `#[serial]` usage, and left `analytics/tests/sql_view_test.rs` / `histo_view_test.rs` as `#[ignore]`d
  integration tests requiring a live Postgres/object-store. This plan builds on that state rather than
  re-litigating it.
- Rust has 18 crates with `tests/` directories; the bulk of the suite (by test count) lives in
  `analytics` (180) and `datafusion-extensions` (145), followed by `analytics-web-srv` (97).
- Python (`python/micromegas/tests/`) has ~130 `def test_*` functions across 27 files. Every file that
  imports `python/micromegas/tests/test_utils.py` triggers `client = micromegas.connect()` at **module
  import time**, so the whole suite fails at pytest *collection*, not just execution, unless a live
  stack (`start_services.py`) is running. There is effectively no unit/integration split despite some
  files being named `*_integration`/`*_e2e`.

## Findings

Organized by the issue's five categories. Each item below was verified by reading the actual test code
(and, where feasible, running it), not just pattern-matched — false positives from the initial grep
sweep are called out explicitly so they aren't re-flagged in a future audit.

### Dead/obsolete coverage

- No `assert!(true)`-style placeholder tests found in Rust or Python.
- **Confirmed dead code, not just a dead test**: `rust/http-gateway/src/config.rs` defines its own
  `HeaderForwardingConfig` struct and 4 tests, but the file is never `mod`-declared anywhere in the
  `http-gateway` crate (that crate has no `lib.rs`, only a `[[bin]] http-gateway-srv` target whose root
  file `src/http_gateway_srv.rs` uses `servers::http_gateway::HeaderForwardingConfig` from the
  `micromegas` (`public`) crate instead, and does not reference `config.rs` at all — verified via
  `grep` for `mod config`/`#[path]` and reading `http_gateway_srv.rs`). This is an orphaned leftover,
  most likely from when the header-forwarding logic was consolidated into `public::servers::http_gateway`
  (commit `44a39d6d4` added the original `http-gateway` crate). The file is not compiled into any
  target — see Implementation Steps for the fix.
- A deeper, non-grep pass through individual test bodies (to find assertions on behavior that's since
  been simplified away) was judged out of scope for a single audit pass — see Open Questions.

### Redundant/duplicate tests

- **Confirmed real duplication**: the 4 tests in `rust/http-gateway/src/config.rs`
  (`test_default_config`, `test_prefix_matching`, `test_blocked_overrides_allowed`,
  `test_case_insensitive`) are byte-for-byte identical to the first 4 tests in
  `rust/public/tests/http_gateway_tests.rs` — expected, since both target the same
  `HeaderForwardingConfig` type (once directly, once via the crate that's actually compiled). Same fix
  as above: deleting the dead file removes the duplication too.
- **Investigated and ruled out** (same test name, different behavior — not redundant):
  - `test_empty_array` in `datafusion-extensions/tests/jsonb_array_elements_tests.rs:84` vs
    `jsonb_array_length_tests.rs:43` — different UDFs (`jsonb_array_elements` vs `jsonb_array_length`),
    bodies differ, both needed.
  - `test_empty_properties` in `analytics/tests/properties_to_jsonb_tests.rs:107` (tests the
    `PropertiesToJsonb` UDF directly) vs `properties_column_accessor_tests.rs:136` (tests the column
    accessor built from JSONB-encoded properties) — different code paths, both needed.
  - `test_valid_api_key`/`test_invalid_api_key` in `auth/tests/api_key_tests.rs` (tests
    `ApiKeyAuthProvider` directly) vs `auth/tests/axum_tests.rs` (tests the same scenarios through the
    axum `auth_middleware`) — intentional layered testing (provider vs middleware), both needed.

### Slow tests

- `public/tests/graceful_shutdown_tests.rs::axum_grace_cap_enforced` has a 10-second sleep in its
  handler, which the initial survey flagged as likely dominating the crate's wall-clock time. **Verified
  false alarm**: ran it in isolation (`cargo test -p micromegas --test graceful_shutdown_tests --features
  server axum_grace_cap_enforced`) — it completes in **0.31s**, because the 10s-sleeping handler task is
  spawned and never awaited to completion; the test only awaits `serve.await`, which returns once the
  200ms grace period elapses. No action needed.
- The two `#[ignore]`d Postgres/object-store tests in `analytics` (`sql_view_test.rs`,
  `histo_view_test.rs`) are already correctly excluded from default `cargo test` runs. No action.
- **Confirmed inconsistency**: `rust/ingestion/tests/readiness.rs::check_ready_returns_true_when_dependencies_healthy`
  silently no-ops (prints a message and returns without asserting anything) when
  `MICROMEGAS_SQL_CONNECTION_STRING` is unset, instead of using `#[ignore]` like the `analytics` crate's
  equivalent live-dependency tests. This means the test *looks* like it ran and passed in every CI run,
  when it never actually exercised anything — a masked gap, and an inconsistent convention with
  `analytics`. See Implementation Steps.

### Flaky / hardcoded-sleep patterns

- Surveyed every `sleep(` call in the Rust and Python test suites. The great majority are deliberate:
  simulating handler latency to create a race window (`graceful_shutdown_tests.rs`,
  `file_cache_tests.rs::test_thundering_herd_single_load`), or measuring elapsed time for span-duration
  assertions (`async_span_tests.rs`) — these are the behavior under test, not a "hope it's done by now"
  wait, so no action needed.
- `object-cache/tests/foyer_backend_tests.rs` already carries a self-authored comment (no actual sleep
  present) warning against introducing a hardcoded-sleep wait for background disk activity if one is
  ever needed — flagging so a future audit doesn't rediscover the same non-issue.
- No genuine "sleep then assert on background/async state that might not have settled yet" pattern was
  found that needs fixing.

### Coverage gaps

- The Python suite's lack of a unit/integration split (see Current State) is a real gap: there is no
  way to run *any* Python test without a live stack, and pytest fails at collection rather than a clean
  per-test skip. Fixing this well (a lazy-connecting fixture, explicit unit/integration markers) touches
  the shared `test_utils.py` and, transitively, every file that imports it (~20 files) — a much larger,
  separate effort with its own risk profile. Flagged as a follow-up, not implemented here (Open
  Questions).
- No other coverage gaps were surfaced by static analysis; identifying *missing* coverage reliably
  needs domain knowledge of what the code is supposed to do, which is better exercised per-feature-PR
  than as a blanket grep-based audit.

## Design — Convention Cleanup

Beyond the dead file, 4 more files violate the CLAUDE.md "tests under `tests/`, not inline" convention.
All of them test only items that are already `pub`, so the bodies can move unchanged apart from import
paths:

| File | Test(s) | Notes |
|---|---|---|
| `rust/tracing/src/time.rs` | `test_frequency` | Tests `pub fn frequency()` — move as-is. |
| `rust/tracing/src/logs/events.rs` | `test_filter_levels` | Tests `LogMetadata`/`FilterState`/`FILTER_LEVEL_UNSET_VALUE` (all re-exported `pub` from `micromegas_tracing::logs`) and `Level`/`LevelFilter`. The existing test imports the latter two via the private path `crate::logs::events::{Level, LevelFilter}`; the moved version must import them from their real public home, `micromegas_tracing::levels::{Level, LevelFilter}`. |
| `rust/tracing/src/string_id.rs` | `test_string_id` | Tests `pub struct StringId` and `InProcSerialize` (from `micromegas_transit`) — move as-is. |
| `rust/micromegas-proc-macros/src/lib.rs` | 10 tests for `expand_micromegas_main` | `expand_micromegas_main` is currently a private `fn`. It must become `pub fn` (consistent with this project's preference for plain `pub` over `pub(crate)`) so an external `tests/` crate can call it. Only the `#[proc_macro] fn micromegas_main` entry point needs to stay macro-only; the token-stream-transform helper it delegates to is fine as a normal public function callable from integration tests. |

`rust/http-gateway/src/config.rs` is **not** moved — see Implementation Steps, it's deleted outright
since it's dead code.

## Implementation Steps

1. **Delete the orphaned dead file in `http-gateway`**
   - Delete `rust/http-gateway/src/config.rs` entirely (struct, `Default`/`from_env`/`should_forward`
     impls, and its 4 tests) — confirmed unreferenced by any `mod` declaration or `#[path]` attribute
     in the crate; the binary uses `micromegas::servers::http_gateway::HeaderForwardingConfig` instead,
     which is already covered by `rust/public/tests/http_gateway_tests.rs`.

2. **Move `tracing` crate inline tests to `tests/`**
   - `rust/tracing/src/time.rs`: remove the `#[cfg(test)] mod tests` block; add
     `rust/tracing/tests/time_tests.rs` with `test_frequency`, calling `micromegas_tracing::time::frequency()`.
   - `rust/tracing/src/logs/events.rs`: remove the `#[cfg(test)] mod test` block; add
     `rust/tracing/tests/log_events_tests.rs` with `test_filter_levels`, importing `LogMetadata`,
     `FilterState`, `FILTER_LEVEL_UNSET_VALUE` from `micromegas_tracing::logs::*` and `Level`,
     `LevelFilter` from `micromegas_tracing::levels::*`.
   - `rust/tracing/src/string_id.rs`: remove the `#[cfg(test)] mod test` block; add
     `rust/tracing/tests/string_id_tests.rs` with `test_string_id`, importing `StringId` from
     `micromegas_tracing::string_id::StringId` and `InProcSerialize` from `micromegas_transit`.

3. **Move `micromegas-proc-macros` inline tests to `tests/`**
   - In `rust/micromegas-proc-macros/src/lib.rs`, change `fn expand_micromegas_main` to
     `pub fn expand_micromegas_main`.
   - Remove the `#[cfg(test)] mod tests { ... }` block.
   - Add `rust/micromegas-proc-macros/tests/expand_micromegas_main_tests.rs` with the same test bodies,
     calling `micromegas_proc_macros::expand_micromegas_main` and importing `quote::quote` /
     `proc_macro2::TokenStream` as the original did.

4. **Fix inconsistent live-dependency gating in `ingestion`**
   - In `rust/ingestion/tests/readiness.rs`, mark `check_ready_returns_true_when_dependencies_healthy`
     with `#[ignore]` (short comment: requires `MICROMEGAS_SQL_CONNECTION_STRING`), matching
     `analytics/tests/sql_view_test.rs` / `histo_view_test.rs`. Drop the silent early-return: since
     `#[ignore]` already keeps it out of default runs, the test body can call
     `WebIngestionService::from_env().await.expect(...)` directly instead of going through
     `try_create_service()`'s `Option`-returning env-var check. Remove `try_create_service` if it ends
     up unused.

5. **Verify**: from `rust/`, run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and
   `cargo test --workspace` (the relocated tests should still pass from their new locations; the
   `ingestion` test should now show as ignored rather than passing).

## Files to Modify

- `rust/http-gateway/src/config.rs` — delete
- `rust/tracing/src/time.rs` — remove inline test
- `rust/tracing/tests/time_tests.rs` — new
- `rust/tracing/src/logs/events.rs` — remove inline test
- `rust/tracing/tests/log_events_tests.rs` — new
- `rust/tracing/src/string_id.rs` — remove inline test
- `rust/tracing/tests/string_id_tests.rs` — new
- `rust/micromegas-proc-macros/src/lib.rs` — `pub fn`, remove inline tests
- `rust/micromegas-proc-macros/tests/expand_micromegas_main_tests.rs` — new
- `rust/ingestion/tests/readiness.rs` — convert to `#[ignore]`, drop dead helper if unused

## Trade-offs

- Considered giving `http-gateway` its own `tests/config_tests.rs` instead of deleting the file, so the
  crate has coverage independent of `public`. Rejected: the crate doesn't even compile `config.rs`
  today (it's dead), and the binary only ever uses the `public` crate's copy — adding tests for code
  the crate doesn't use would just be more dead weight. If `http-gateway` ever needs logic of its own
  again, it should gain fresh code and fresh tests at that point.
- Considered a broader Python test-harness rework (lazy-connect fixture, unit/integration pytest
  markers) to close the coverage-gap finding directly. Rejected for this plan: no live bug motivates it,
  the blast radius (~20 files) is out of proportion to a "review the suite" issue, and it deserves its
  own scoped plan with its own review.
- Considered leaving `ingestion/tests/readiness.rs`'s silent-skip pattern alone since it never actually
  fails. Rejected: it currently reports as "passed" in CI without ever exercising the code path, which
  is a more misleading state than an honest `#[ignore]` (which shows up distinctly in test-run summaries)
  — and `#[ignore]` is already this repo's convention for the same situation in `analytics`.

## Testing Strategy

- `cargo test --workspace` (from `rust/`) after the moves: same pass count as before minus the one test
  newly marked `#[ignore]`, with the four relocated tests (`time_tests`, `log_events_tests`,
  `string_id_tests`, `expand_micromegas_main_tests`) passing from their new locations.
- `cargo clippy --workspace -- -D warnings` and `cargo fmt` must stay clean (deleting a whole file and
  changing one `fn` to `pub fn` shouldn't introduce new lints, but confirm).
- No Python changes in this plan, so no Python test run is required beyond existing CI.

## Open Questions

- Should the Python test suite's all-tests-require-a-live-server structure be split into a true
  unit/integration tier as a follow-up issue? This plan documents the finding but does not implement a
  fix, given the wide blast radius relative to this issue's scope.
- Should a deeper, manual (non-grep) pass through individual Rust/Python test bodies be scheduled to
  catch dead/obsolete assertions that don't match trivial patterns (e.g. tests asserting on behavior for
  a code path that was since simplified away)? Nothing like that was found here, but grep-based
  detection can't rule it out.
