# Update DataFusion to 54.1.0 Plan

## Overview
Bump the `datafusion` dependency from `54.0` to `54.1` across the two Rust trees that declare it directly — the main workspace (`rust/Cargo.toml`) and the standalone `datafusion-wasm` crate (`rust/datafusion-wasm/Cargo.toml`) — and regenerate both lockfiles and the checked-in WASM bindings. This is a patch release within the same DataFusion/Arrow line, so the change is expected to be low-risk relative to the 50→51 upgrade (see Current State), but every dependent surface still needs to be re-verified.

## Current State
- `rust/Cargo.toml:50` declares `datafusion = "54.0"` as the single workspace-level dependency; every other crate (`analytics`, `datafusion-extensions`, `perfetto`, `analytics-web-srv`, `public`, `examples/write-perfetto`) references it via `datafusion.workspace = true`, so only this one line needs to change for the main workspace.
- `rust/datafusion-wasm/Cargo.toml:20` is **excluded** from the main workspace (`rust/Cargo.toml:3`) and declares its own `datafusion = { version = "54.0", default-features = false, features = ["nested_expressions", "sql"] }`, with its own `Cargo.lock`. It also pins `arrow = { version = "58.0", ... }` directly (line 19).
- `build/check_wasm_deps.py` is a CI gate (invoked as the first step of `run_wasm()` in `build/rust_ci.py`) that fails the build if a dependency name shared between `rust/Cargo.toml`'s `[workspace.dependencies]` and `rust/datafusion-wasm/Cargo.toml` has a differing version *string* (exact string match, not semver-compatible match). `datafusion` is one such shared name, so both files must be bumped to the identical version string.
- Verified via crates.io and the local `~/.cargo/registry` cache that both `datafusion 54.0.0` and `datafusion 54.1.0` depend on `arrow`/`arrow-schema` `^58.3.0` — **the Arrow version is unchanged** by this bump. `rust/Cargo.lock` currently resolves `arrow`/`parquet` to `58.3.0` already. This matters because the prior DataFusion 50→51 upgrade broke on-disk Parquet metadata compatibility specifically because it crossed an Arrow major version (56→57, see `tasks/completed/datafusion51_metadata_bug.md` and `tasks/completed/datafusion51_partition_format_versioning.md`, which added the `partition_format_version` column/dispatch in `rust/analytics/src/lakehouse/partition_metadata.rs` and `write_partition.rs`). Since 54.0→54.1 does **not** cross an Arrow version boundary, that versioning mechanism does not need a new version added and no metadata-compat work is expected.
- Per the upstream changelog (`apache/datafusion` tag `54.1.0`, 19 commits / 9 contributors), 54.1.0 contains **no documented breaking changes** — only bug fixes (join null-awareness/outer-join elimination, `width_bucket`/`array_compact` edge cases, subquery schema errors, recursive CTE nullability, file-group-statistics panics, regex simplification, `approx_distinct` overcounting, higher-order UDF null coercion) plus a new `enable_file_stream_work_stealing` config option and a Parquet page-index pruning optimization. None of these touch code this repo customizes — a grep for `impl ExecutionPlan for` / `OptimizerRule for` / `UserDefinedLogicalNode` found no matches in `datafusion-extensions/src`; the only `ExecutionPlan` implementors in `analytics/src` are `partitioned_table_provider.rs`, `perfetto_trace_execution_plan.rs`, `materialized_view.rs`, `process_spans_table_function.rs`, and `dfext/task_log_exec_plan.rs`, none of which touch joins, CTEs, `width_bucket`/`array_compact`, or file-group statistics.
- `build/rust_ci.py`'s `run_wasm()` includes a bindings-freshness check (`python3 build.py --check` in `datafusion-wasm/`), which fails if the checked-in generated output under `analytics-web-app/src/lib/datafusion-wasm/` doesn't match a fresh `wasm-pack build`. A prior Rust toolchain bump changed generated closure-glue symbol names and required regenerating these bindings (see `CHANGELOG.md` "Build" entry, "Bump the pinned Rust toolchain to 1.97.0"); a DataFusion patch bump is a smaller surface but the check must still be re-run since `micromegas-datafusion-extensions` (used by the wasm crate) depends on `datafusion.workspace`-shaped types.
- Neither `rust/deny.toml` nor `rust/Cargo.toml`'s `[patch]`/`[replace]` sections (none exist) pin `datafusion`/`arrow`/`parquet` by version, so there's nothing else to keep in sync.

## Design
Straightforward two-file dependency bump plus the standard regeneration/verification steps `check_wasm_deps.py` and the WASM bindings-freshness check already enforce in CI. No code changes are anticipated given the changelog shows no breaking API changes and no Arrow version crossing; the plan budgets time for fixing compile/test fallout only if it materializes.

## Implementation Steps

1. **Bump the main workspace**
   - `rust/Cargo.toml:50`: `datafusion = "54.0"` → `datafusion = "54.1"`.
   - From `rust/`, run `cargo update -p datafusion` (or a full `cargo update` if other advisories are pending) to pull `datafusion-*` subcrates to `54.1.0` in `rust/Cargo.lock`; confirm `arrow`/`arrow-schema`/`parquet` stay at `58.3.0` (per Current State, they should not move).
   - `cargo build --workspace` and `cargo test --workspace` to catch any fallout from the bug fixes listed in Current State (in particular, if any existing test relies on the pre-fix buggy behavior — e.g. the outer-join-elimination or recursive-CTE-nullability fixes — it may need updating rather than the code).

2. **Bump `datafusion-wasm` in lockstep**
   - `rust/datafusion-wasm/Cargo.toml:20`: `datafusion = { version = "54.1", ... }` — the version string must exactly match step 1's `rust/Cargo.toml` value or `build/check_wasm_deps.py` fails CI.
   - From `rust/datafusion-wasm/`, run `cargo update -p datafusion` to refresh its independent `Cargo.lock`; confirm `arrow` stays resolvable under the existing `"58.0"` pin (no change expected there, per Current State).
   - Run `python3 build/check_wasm_deps.py` from the repo root to confirm the version strings match.

3. **Regenerate and verify WASM bindings**
   - From `rust/datafusion-wasm/`, run `python3 build.py --test` then `python3 build.py` (or whatever regenerates `analytics-web-app/src/lib/datafusion-wasm/`), and diff the output. Commit any changes if the generated bindings differ.
   - Run `python3 build.py --check` to confirm freshness (this is what CI's `run_wasm()` step runs) — this must pass before considering the bump complete.

4. **Run full CI locally**
   - `./build/rust_ci.py native` and `./build/rust_ci.py wasm` (per `.github/workflows/rust.yml`), which cover `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo machete`, `cargo audit`, `cargo deny check licenses bans sources` (run against both the main tree and the wasm tree), and `cargo test`.
   - Run the Python integration test suite that previously caught the 50→51 metadata regression (`test_blocks_query`, `test_blocks_properties_stats` — see `tasks/completed/datafusion51_metadata_bug.md`) against a running local stack (`python3 local_test_env/ai_scripts/start_services.py`) to double-check no metadata compatibility issue slipped through despite the Arrow-version analysis above.

5. **Update CHANGELOG**
   - Add a bullet under `## Unreleased` → `**Build:**` in `CHANGELOG.md`, following the existing dependency-bump bullet style (e.g. the Grafana SDK / react-router bump entries already there): "Bump `datafusion` from 54.0 to 54.1.0 (bug-fix release; no Arrow version change)."

## Files to Modify
- `rust/Cargo.toml` — `datafusion = "54.0"` → `"54.1"`
- `rust/Cargo.lock` — resolved `datafusion-*` subcrates to `54.1.0`
- `rust/datafusion-wasm/Cargo.toml` — `datafusion` version string, kept in sync with the workspace
- `rust/datafusion-wasm/Cargo.lock` — resolved `datafusion-*` subcrates to `54.1.0`
- `analytics-web-app/src/lib/datafusion-wasm/**` — regenerated WASM bindings, only if `build.py --check` reports drift
- `CHANGELOG.md` — new `Unreleased`/`Build` bullet

## Trade-offs
- **`cargo update -p datafusion` vs. a full `cargo update`**: scoping the update to just `datafusion` (and its transitive subcrates, which Cargo pulls along automatically) keeps the diff minimal and avoids incidentally bumping unrelated dependencies in the same PR. A full `cargo update` is left as a separate, independent task if desired.
- **No new `partition_format_version` value**: considered whether to preemptively add a Version 3 per the "Future Considerations" note in `tasks/completed/datafusion51_partition_format_versioning.md`, but since Arrow does not change version in this bump, there's no new on-disk format to version — doing so would be speculative and is skipped.

## Testing Strategy
- `cargo test --workspace` (main tree) and `python3 build.py --test` (wasm tree) for unit/integration coverage.
- `./build/rust_ci.py native` and `./build/rust_ci.py wasm` to mirror CI exactly, including the `check_wasm_deps.py` version-sync gate and the `build.py --check` bindings-freshness gate.
- Manual/integration pass against a running local stack (`local_test_env/ai_scripts/start_services.py`) exercising `micromegas-query` against `blocks_view`/`log_entries`/materialized views, since Parquet metadata reading is the one area with prior-incident history for DataFusion bumps.

## Open Questions
None — this is a same-Arrow-line patch bump with a changelog showing no breaking changes and no customized DataFusion trait implementations in the affected code paths.
