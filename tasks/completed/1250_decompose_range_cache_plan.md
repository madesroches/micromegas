# Decompose `range_cache.rs` Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1250

## Overview

`rust/object-cache/src/range_cache.rs` is 1290 lines — the structural outlier of
the workspace — and its `fetch_blocks` method is ~294 lines (lines 702–995).
This plan splits the file into cohesive submodules under a `range_cache/`
directory and decomposes `fetch_blocks` into smaller, named helpers. This is a
pure **refactor**: no behavior changes, identical public API, identical metrics,
same tests passing, and `cargo clippy --workspace -- -D warnings` clean.

Per the issue's own guidance ("best done as several small, independently
reviewable PRs rather than one large one," prioritized by leverage with
`range_cache.rs` as item #1), this plan covers **only** `range_cache.rs` — the
highest-leverage, self-contained unit. The other listed functions live in
different crates and are deferred to follow-up PRs (see [Scope](#scope-what-this-pr-does-not-touch)).

## Scope: what this PR does NOT touch

The issue lists four other decomposition candidates. They are **out of scope**
here and each should be a separate follow-up PR (issue #1250 can stay open until
they land):

| Function | Location | Follow-up |
|----------|----------|-----------|
| `get_range_handler_inner` / `post_ranges_handler_inner` | `object-cache-srv/src/handlers.rs` | PR 2 |
| `execute_query` | `public/src/servers/flight_sql_service_impl.rs` | PR 3 |
| `insert_partition` | `analytics/src/lakehouse/write_partition.rs` | PR 4 |
| `auth_callback` | `analytics-web-srv/src/auth.rs` | handled by the auth-crate refactor issue (per issue note) |

Keeping this PR to one crate and one file keeps it behavior-preserving and
independently reviewable.

## Current State

`range_cache.rs` (single file, 1290 lines) contains, in order:

- **Errors / caller enum** — `RangeError` (lines 20–26); `StreamRangesCaller`
  enum + its `emit_error_metric` (28–50).
- **Constants** — `DEFAULT_BLOCK_SIZE`, `DEMAND_WINDOW_BLOCKS`,
  `DEFAULT_TOTAL_FETCH_PERMITS`, `DEFAULT_DEMAND_RESERVED_FETCH_PERMITS`,
  `DEFAULT_MAX_COALESCED_GET_BYTES`, `DEFAULT_PROMOTE_WHOLE_BATCH` (public,
  52–72); `BACKEND_PROBE_CONCURRENCY` (private, 77).
- **Priority** — `Priority` enum + `from_u8`/`class_label` (79–103);
  `effective_priority` free fn (109–115).
- **Single-flight scheduler** — `BatchState` (126–128); `FetchResult` alias
  (130); `InFlight` struct + impl (132–197); `Ownership` enum (199–202);
  `FetchScheduler` struct + impl (`own_or_join`, `promote_batch_siblings`,
  `remove_entry`, `fetch_budget_stats`, `inflight_len`) (204–335);
  `FulfillGuard` + `Drop` (337–389); `RunPermit` (391–396); `any_entry_promoted`
  (403–415); `acquire_run_permit` (421–456).
- **Small helpers** — `reconstruct_shared_error` (458–479); `decode_size`
  (481–485).
- **`RangeCache`** struct (487–530) + impl (532–1248): `new`,
  `with_prefix_labels`, `classify_tags`, `classify`, `fetch_budget_stats`,
  `inflight_len`, `backend_disk_stats`, `block_size`, `size`, **`fetch_blocks`**,
  `stream_ranges`, `stream_ranges_with_size`, `stream_ranges_inner`,
  `get_range`, `get_range_with_size`, `get_ranges`, `get_ranges_with_size`,
  `prefetch_ranges`, `prefetch_blocks`.
- **`collect_ranges_from_stream`** free fn (1250–1290).

### Public API surface that MUST be preserved

All external users reference the crate through the `range_cache::` module path.
Verified consumers (`object-cache-srv/src/{app_state,cli,handlers,object_cache_srv,prefetch_queue,saturation_monitor}.rs`,
`object-cache/src/l1_store.rs`, and the tests under
`object-cache/tests/` and `object-cache-srv/tests/`) import only these public
items, which must remain re-exported from `range_cache::` unchanged:

- Types: `RangeCache`, `RangeError`, `StreamRangesCaller`
- Constants: `DEFAULT_BLOCK_SIZE`, `DEFAULT_TOTAL_FETCH_PERMITS`,
  `DEFAULT_DEMAND_RESERVED_FETCH_PERMITS`, `DEFAULT_MAX_COALESCED_GET_BYTES`,
  `DEFAULT_PROMOTE_WHOLE_BATCH`, `DEMAND_WINDOW_BLOCKS`
- Every existing `pub` method of `RangeCache`.

No consumer imports `Priority`, `InFlight`, `FetchScheduler`, etc. — those are
private today and stay module-internal, so they can move freely between
submodules using `pub(crate)`/`pub(super)` visibility as needed.

### `fetch_blocks` internal structure (the ~294-line method)

Reading lines 702–995, `fetch_blocks` is four sequential phases:

1. **Classify + probe** (712–784): classify tags once; probe every requested
   block against the backend with bounded concurrency; partition into `hits`
   (demand only) and `missing`, healing length-mismatched cached blocks by
   treating them as missing. Early-return `hits` if nothing missing; then
   `sort/dedup` missing.
2. **Register in-flight** (786–817): build the optional `BatchState` (prefetch
   only); `own_or_join` every missing block to split into `owned` vs joined;
   collect `entries` map; compute `FillHint`.
3. **Spawn coalesced run fetches** (819–956): for each run from
   `coalesce_runs(owned, …)`, spawn a detached task that acquires a run permit,
   issues one `origin.get_range`, validates the run length, splits the buffer
   into per-block chunks, writes each to the backend, fulfills each entry, and
   removes keys from the scheduler (with a `FulfillGuard` for the panic path).
4. **Join** (958–994): prefetch path joins via `FuturesUnordered`, drops bytes,
   returns empty map; demand path `join_all`s in index order and returns the
   populated `hits` map.

## Design

### File layout

Convert the single file into a module directory. In edition 2024 a `foo.rs`
file may coexist with a `foo/` directory holding its submodules, so **no change
to `lib.rs`** is required (`pub mod range_cache;` still resolves) — but the
cleaner, unambiguous form is to move the file to `range_cache/mod.rs`. This plan
uses `range_cache/mod.rs`.

```
object-cache/src/range_cache/
  mod.rs        RangeCache struct + its public methods + pub re-exports of the
                submodule items that form the crate-facing API; the struct's
                doc comment travels with it.
  error.rs      RangeError, StreamRangesCaller (+ emit_error_metric).
  scheduler.rs  Priority, effective_priority, BatchState, FetchResult, InFlight,
                Ownership, FetchScheduler, FulfillGuard, RunPermit,
                any_entry_promoted, acquire_run_permit, reconstruct_shared_error,
                decode_size.
  fetch.rs      `impl RangeCache` block holding `fetch_blocks` (`pub(super)`,
                since it's called from `mod.rs`) decomposed into private
                sub-helper methods (see below), plus BACKEND_PROBE_CONCURRENCY.
                Child module of range_cache, so it can access RangeCache's
                private fields.
```

Import path note: `range_cache.rs` today reaches its sibling top-level modules
via `super::backend`, `super::blocks`, and `super::metric_tags` (lines 16–18),
which resolves because `super` = crate root for a module declared directly in
`lib.rs`. Once that code moves into `range_cache/scheduler.rs` and
`range_cache/fetch.rs`, `super` instead resolves to the `range_cache` module,
so those paths must be rewritten to `crate::backend`, `crate::blocks`, and
`crate::metric_tags`. `mod.rs` itself is unaffected — for it, `super` still
means the crate root, so its own `super::` imports stay unchanged.

Rationale for this grouping:
- `error.rs` is the tiny, self-contained public error/caller surface.
- `scheduler.rs` is the cohesive single-flight + permit machinery
  (`FetchScheduler`, `InFlight`, promotion, permits) plus `decode_size` and
  `reconstruct_shared_error`, two small pure helpers shared across submodules
  (`decode_size` by `size()` in `mod.rs`; `reconstruct_shared_error` by
  `size()` in `mod.rs` and by `fetch.rs`) that are grouped here rather than
  given their own file. This is the bulk of the non-`RangeCache` code and has
  a clear single responsibility.
- `fetch.rs` isolates the one giant method and its new sub-helpers, so `mod.rs`
  stays focused on the cache's public surface and the streaming/assembly logic.
- Constants (`DEFAULT_*`, `DEMAND_WINDOW_BLOCKS`) stay in `mod.rs` (they are the
  public knobs and belong with the type), re-exported implicitly by being `pub`
  at module root. `BACKEND_PROBE_CONCURRENCY` lives in `fetch.rs`, since
  `fetch_blocks` is its sole user.

### Visibility

Items moved out of `mod.rs` but used across submodules become `pub(super)` (or
`pub(crate)` where an ancestor beyond the parent needs them — none currently do,
so `pub(super)` suffices for everything except the already-`pub` API). The
already-`pub` items (`RangeError`, `StreamRangesCaller`, the `DEFAULT_*`
consts) are re-exported from `mod.rs` with `pub use` so the `range_cache::`
path is unchanged for external callers.

In Rust, marking a type `pub(super)` changes only the visibility of the type
itself — it does **not** change the visibility of any of its inherent methods.
Each inherent method carries its own independent visibility modifier, so every
currently-private method that ends up called across the new module boundary
must be marked `pub(super)` individually, in addition to the type. Concretely,
these inherent methods are called from `mod.rs` and/or `fetch.rs` and each
needs its own `pub(super)` in `scheduler.rs`: `FetchScheduler::{new,
own_or_join, remove_entry, fetch_budget_stats, inflight_len}`,
`InFlight::{fulfill, join}`, `FulfillGuard::{new, disarm}`, and
`Priority::class_label`.

`RunPermit` itself needs the same treatment for a different reason: it is the
one moved type whose only cross-module use is as `acquire_run_permit`'s return
type, so it must be `pub(super)` right alongside that function — without it,
rustc reports `RunPermit` as private at every call site in `fetch.rs`, even
though `RunPermit` is never named explicitly there (the permit is bound by
inference and dropped).

The definitive rule behind all of the above: any item — type, free fn,
inherent method, or field — referenced from another submodule, including only
as a parameter or return type, must be `pub(super)`; the build-after-each-step
compilation gate in the Implementation Steps is the authoritative backstop
that will surface any visibility gap this enumeration misses.

Separately, `pub(super)` on a type also does not cover field-literal
construction of its fields from another module. `BatchState` is the one item
here built via a bare field literal (`BatchState { entries: StdMutex::new(...) }`),
and that construction site only moves into `fetch.rs`'s `register_missing`
helper in Step 4 — at the end of Step 3, `fetch_blocks` (the sole call site)
still lives in `mod.rs`. So the `entries`-private + constructor change is
deferred to Step 4: in Step 3, `BatchState` moves to `scheduler.rs` but its
`entries` field stays `pub(super)`, so the still-in-`mod.rs` field literal
keeps compiling. In Step 4, once the field literal relocates into
`register_missing`, `scheduler.rs` gives `BatchState` a `pub(super) fn
new(...) -> Self` constructor, `register_missing` calls `BatchState::new(...)`
instead of the field literal, and `entries` becomes private (consistent with
every other scheduler type, which is constructed via `::new`/internal fns). No
other moved item is constructed by field literal across the
`fetch.rs`/`scheduler.rs` boundary, so this is the only extra visibility
consideration beyond `pub(super)` on types/fns/methods.

Concretely, `mod.rs` will contain:
```rust
mod error;
mod fetch;
mod scheduler;

pub use error::{RangeError, StreamRangesCaller};
// scheduler internals are pub(super)/pub(crate); nothing re-exported publicly.
```
`fetch.rs` contributes methods to `RangeCache` via a second
`impl RangeCache { … }` block; splitting an inherent impl across files in the
same module tree is allowed.

### `fetch_blocks` decomposition

Break the method into the four phases, each a private helper on `RangeCache`
(or a free fn where no `&self` state is needed), so the orchestrator
`fetch_blocks` drops to well under 80 lines and each helper is cohesive and
independently readable:

- `struct ProbeOutcome { hits: HashMap<u64, Bytes>, missing: Vec<u64> }` — small
  return type for phase 1 (avoids a bare tuple).
- `async fn probe_blocks(&self, key, file_size, indices, prio, block_tag) -> ProbeOutcome`
  — phase 1: bounded-concurrency backend probe, length-mismatch healing,
  hits/missing partition, sort+dedup of `missing`. Returns early-empty-missing
  naturally (caller checks `missing.is_empty()`).
- `fn register_missing(&self, key, missing: &[u64], prio) -> (Vec<u64> /*owned*/, HashMap<u64, Arc<InFlight>>, Option<Arc<BatchState>>)`
  — phase 2: build `BatchState` via its `BatchState::new(...)` constructor (see
  [Visibility](#visibility)), `own_or_join` each missing block, return `owned`
  + `entries` + `batch`. Takes `missing` by reference (rather than by value) so
  `fetch_blocks` retains ownership of the `Vec<u64>` for the `join_demand` call
  in phase 4.
- `fn spawn_run_fetch(&self, key, file_size, run, run_entries, run_keys, hint, run_class_tags…)`
  — phase 3: the per-run detached task body (permit acquire → origin GET →
  length check → chunk split → backend put + fulfill → scheduler cleanup, with
  `FulfillGuard`). Called once per `coalesce_runs` run. This is the largest
  extracted piece; keep its `tokio::spawn` closure body tight by moving the
  success-path chunk-splitting into a small inner helper if it still exceeds
  ~80 lines.
- `async fn join_demand(entries, missing, hits) -> Result<HashMap<u64, Bytes>>`
  and `async fn join_prefetch(entries) -> Result<()>` — phase 4's two join
  strategies (free fns; they need no `&self`).

`fetch_blocks` then reads as: classify → `probe_blocks` → early return →
`register_missing` → per-run `spawn_run_fetch` → `join_demand`/`join_prefetch`.

`fetch_blocks` itself is called from `stream_ranges_inner` and
`prefetch_blocks`, both of which stay in `mod.rs`; since a private item in a
child module isn't visible to its parent, `fetch_blocks` must be marked
`pub(super)` (like the scheduler items in Step 3). The extracted sub-helpers
(`probe_blocks`, `register_missing`, `spawn_run_fetch`, `join_demand`,
`join_prefetch`) are called only from within `fetch.rs` and stay private.

Metric emission (`imetric!`/`fmetric!`), tag classification, and the exact
control flow (early returns, sort/dedup, prefetch-vs-demand branching, the
`FulfillGuard`/panic path, detached-task spawning) are preserved verbatim —
only relocated. No signatures of public methods change.

## Implementation Steps

1. **Create the directory and move the file.**
   `git mv object-cache/src/range_cache.rs object-cache/src/range_cache/mod.rs`
   (preserves history). Confirm `cargo build -p micromegas-object-cache` still
   compiles unchanged before splitting.
2. **Extract `error.rs`.** Move `RangeError` and `StreamRangesCaller` (+
   `emit_error_metric`). Add `mod error; pub use error::{RangeError, StreamRangesCaller};`
   to `mod.rs`. `emit_error_metric` is currently a private method; its callers
   are `mod.rs` methods, so mark it `pub(super)`. Build.
3. **Extract `scheduler.rs`.** Move `Priority`, `effective_priority`,
   `BatchState`, `FetchResult`, `InFlight`, `Ownership`, `FetchScheduler`,
   `FulfillGuard`, `RunPermit`, `any_entry_promoted`, `acquire_run_permit`,
   `reconstruct_shared_error`, `decode_size`. Mark each moved top-level item
   used from `mod.rs` or `fetch.rs` as `pub(super)`. Marking a type
   `pub(super)` does **not** carry over to its inherent methods — each method
   has independent visibility — so also mark each of these currently-private
   methods `pub(super)` individually, since they are called across the new
   module boundary: `FetchScheduler::{new, own_or_join, remove_entry,
   fetch_budget_stats, inflight_len}`, `InFlight::{fulfill, join}`,
   `FulfillGuard::{new, disarm}`, and `Priority::class_label` (see
   [Visibility](#visibility)). Also mark `RunPermit` itself `pub(super)` — it
   is the one moved type whose only cross-module use is as
   `acquire_run_permit`'s return type, and it is never named explicitly in
   `fetch.rs` (see [Visibility](#visibility)). Keep `BatchState`'s `entries`
   field `pub(super)` for now (not private) — its only construction site is
   still the field literal in `mod.rs`'s `fetch_blocks` at this point
   (`fetch_blocks` doesn't move to `fetch.rs` until Step 4), so making
   `entries` private here would break that field literal before its call site
   is converted to a constructor call (see [Visibility](#visibility)). Add
   `mod scheduler; use scheduler::*;` (or
   explicit `use`s) to `mod.rs`. Since `scheduler.rs` now lives one level
   deeper than `range_cache.rs` did, rewrite its `super::metric_tags` import
   (used by `Priority::class_label` for `CLASS_DEMAND`/`CLASS_PREFETCH`) to
   `crate::metric_tags` (see the File layout section's import path note).
   Build.
4. **Extract `fetch.rs` and decompose `fetch_blocks`.** Move `fetch_blocks`
   into a `impl RangeCache` block in `fetch.rs`, along with
   `BACKEND_PROBE_CONCURRENCY`. Mark `fetch_blocks` `pub(super)`, since it is
   still called from `stream_ranges_inner` and `prefetch_blocks` in `mod.rs`
   (same rule as Step 3's scheduler items). Introduce the helpers from the
   Design section (`ProbeOutcome`, `probe_blocks`, `register_missing`,
   `spawn_run_fetch`, `join_demand`, `join_prefetch`) as private items, since
   they are only called within `fetch.rs`. This step also moves `BatchState`'s
   field-literal construction (`BatchState { entries: StdMutex::new(...) }`)
   into `register_missing`; at this point, add `scheduler.rs`'s `pub(super)
   fn new(...) -> Self` constructor for `BatchState`, switch `register_missing`
   to call `BatchState::new(...)` instead of the field literal, and change
   `entries` from `pub(super)` (set in Step 3) to private (see
   [Visibility](#visibility)). Add `mod fetch;` to `mod.rs`. Like
   `scheduler.rs` in Step 3, `fetch.rs` now lives one level deeper than
   `range_cache.rs` did, so rewrite its `super::blocks` (for
   `block_byte_range`, `coalesce_runs`), `super::backend` (for `FillHint`), and
   `super::metric_tags` (for `class_tags`, `CLASS_DEMAND`) imports to
   `crate::blocks`, `crate::backend`, and `crate::metric_tags` respectively
   (see the File layout section's import path note). Keep every metric name,
   tag, log line, and branch identical. Build.
5. **Verify each helper is < ~80 LOC** and `fetch_blocks` orchestrator is small;
   adjust extraction boundaries if any helper is still oversized (e.g. split the
   run success path as noted).
6. **Format, lint, test** (see Testing Strategy). Run `cargo fmt`, clippy,
   crate tests, then the workspace CI script.

Build after each step so a break is localized to the step that caused it.

## Files to Modify

- `rust/object-cache/src/range_cache.rs` → **moved** to
  `rust/object-cache/src/range_cache/mod.rs`, then trimmed.
- `rust/object-cache/src/range_cache/error.rs` — **new**.
- `rust/object-cache/src/range_cache/scheduler.rs` — **new**.
- `rust/object-cache/src/range_cache/fetch.rs` — **new**.
- `rust/object-cache/src/lib.rs` — **no change** (`pub mod range_cache;` still
  resolves to `range_cache/mod.rs`). Verify, don't edit.

No test files change (the public API is preserved). No other crate changes.

## Trade-offs

- **`range_cache/mod.rs` vs. keeping `range_cache.rs` + a sibling `range_cache/`
  dir.** Edition 2024 permits both, but `mod.rs` is the unambiguous, single-
  location form and avoids a file and a same-named directory sitting side by
  side. Chosen: `mod.rs`.
- **Three submodules vs. finer split.** Could split `scheduler.rs` further
  (e.g. `inflight.rs` + `permits.rs`), but `FetchScheduler`, `InFlight`, and the
  permit logic are tightly coupled (promotion touches all three); one
  `scheduler.rs` keeps that cohesion visible. Chosen: coarser, cohesive split.
- **Helper methods vs. free functions for `fetch_blocks` phases.** Phases that
  read `self` (block size, backend, scheduler, classify) are `&self` methods;
  the two join strategies need no `self` and are free fns. This mirrors the
  existing file's mix (e.g. `acquire_run_permit` is a free fn taking
  `&FetchScheduler`).
- **`git mv` first vs. new-file copy.** `git mv` preserves blame/history on the
  large body that stays in `mod.rs`; the extracted submodules are new files
  regardless.

## Documentation

No user-facing or `mkdocs/` documentation covers `range_cache` internals (it is
an implementation detail of the object cache). The struct-level doc comment on
`RangeCache` (current lines 487–510) is preserved and stays with the struct in
`mod.rs`. No documentation changes required.

## Testing Strategy

This is behavior-preserving, so the existing test suites are the specification:

- `cargo test -p micromegas-object-cache` — includes `range_cache_tests.rs`
  (single-flight, promotion, length-mismatch healing, streaming, prefetch),
  `telemetry_tests.rs`, `l1_store_tests.rs`, `foyer_backend_tests.rs`,
  `metric_tags_tests.rs`.
- `cargo test -p micromegas-object-cache-srv` — the server crate depends on
  the preserved public API (`memory_budget_tests.rs`, `prefetch_tests.rs`,
  `saturation_tests.rs`, `telemetry_tests.rs`).
- `cargo clippy --workspace -- -D warnings` — must be clean (acceptance
  criterion).
- `cargo fmt` — required before commit.
- `python3 ../build/rust_ci.py` from `rust/` — full CI parity (fmt check, clippy,
  tests) as a final gate.

Success = all of the above green with **no test edits**. If any test needs
editing to pass, the refactor changed behavior and must be corrected instead.

## Open Questions

None. The public API to preserve is enumerated and verified against all
in-workspace consumers; the split is internal and mechanical.
