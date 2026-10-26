# Remove frame_budget_reporting Module Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/905

## Overview

Issue #905 asked to rework `rust/public/src/client/frame_budget_reporting.rs` on top of a
cross-thread span table function. The module is not used by anything in the repository, so instead
of reworking dead code this plan deletes it. The Rust API break is recorded in the changelog; the
SQL surface is untouched.

## Current State

- `rust/public/src/client/frame_budget_reporting.rs` (~470 lines) builds per-frame CPU budget
  statistics by querying one stream at a time with `view_instance('thread_spans', '{stream_id}')`.
- It is declared in `rust/public/src/client/mod.rs:7-9` under `#[cfg(feature = "server")]`.
- A repo-wide search (`rust/`, `python/`, `mkdocs/`, `README.md`) finds no other reference to the
  module or its public items (`GroupBy`, `budget_map_to_properties`, `fetch_spans_batch`,
  `extract_top_offenders`, `compute_frame_stats_for_batch`, ...). The only other mentions are in
  `CHANGELOG.md` and historical plans under `tasks/`.
- Every dependency the module imports (`async-stream`, `futures`, `chrono`, `anyhow`, `datafusion`,
  `micromegas-analytics`) is also used by other modules of the `micromegas` crate (for example
  `perfetto_trace_client.rs`), so no `Cargo.toml` change is needed.

## Design

Delete the file and the `pub mod` declaration. No replacement is provided: the module has no
in-repo callers, and `process_spans(process_id, 'thread'|'async')` is available in SQL for anyone
who needs cross-thread span queries.

## Implementation Steps

1. Delete `rust/public/src/client/frame_budget_reporting.rs`.
2. Remove the `frame_budget_reporting` declaration (and its `#[cfg(feature = "server")]` and doc
   comment lines) from `rust/public/src/client/mod.rs`.
3. Add an entry under `## Unreleased` in `CHANGELOG.md` (#905): the
   `micromegas::client::frame_budget_reporting` module is removed. Include a **Minor breaking
   change** clause listing the removed public items. State that no SQL view, table or function
   changes.
4. Run `cargo build --all-features` and `cargo clippy --workspace -- -D warnings` from `rust/` to
   confirm nothing else referenced the module and no import became unused.

## Files to Modify

- `rust/public/src/client/frame_budget_reporting.rs` (delete)
- `rust/public/src/client/mod.rs`
- `CHANGELOG.md`

## Trade-offs

- **Rework onto `process_spans`** (the issue's original ask): rejected, since nothing uses the
  module and the work would maintain code with no consumer.
- **Keep the module as-is**: rejected. It keeps a stale per-stream query path alive, including a
  helper that takes a caller-supplied stream id.

## Documentation

No `mkdocs/` page references the module. The `CHANGELOG.md` entry is the only documentation
change.

## Testing Strategy

The module has no tests of its own and nothing else depends on it, so the compiler is the check:
`cargo build --all-features`, `cargo clippy --workspace -- -D warnings`, and `cargo test` for the
`micromegas` crate from `rust/`.
