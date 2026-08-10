# HistogramAccumulator::size() Under-Reports Memory Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1448

## Overview

`HistogramAccumulator::size()` (`rust/datafusion-extensions/src/histogram/accumulator.rs:310-312`)
reports the `Vec` *header* size for `bins` instead of its allocated capacity, under-reporting
memory usage to DataFusion's memory pool by up to ~7x for small histograms (worse for larger
`nb_bins`). This breaks DataFusion's memory-budget guardrail
(`MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB`) and delays/skips spilling for grouped histogram
aggregates, risking OOM kills instead of clean `ResourcesExhausted` errors.

## Current State

```rust
// rust/datafusion-extensions/src/histogram/accumulator.rs:310-312
fn size(&self) -> usize {
    size_of_val(self) + size_of_val(&self.bins)
}
```

`size_of_val(&self.bins)` returns `size_of::<Vec<u64>>()` (24 bytes: ptr + len + capacity), not
the heap allocation the `Vec` actually owns. DataFusion's `Accumulator::size` contract
(`datafusion-expr-common/src/accumulator.rs`) requires reporting allocated size using `capacity`,
not `len`, for internal containers.

`HistogramAccumulator` (`rust/datafusion-extensions/src/histogram/accumulator.rs:20-29`) holds a
single `Vec<u64>` field (`bins`) sized via `configure_from_params`/`configure`
(`accumulator.rs:60-105`) to `nb_bins`. This is the only heap-allocating field and the only
`Accumulator::size` implementation in the tree (per the issue).

`make_histogram` (`histogram_udaf.rs:221-235`) and `sum_histograms`
(`sum_histograms_udaf.rs:14-23`) are both registered via `create_udaf` without overriding
`groups_accumulator_supported`, so DataFusion boxes one `HistogramAccumulator` per group via
`GroupsAccumulatorAdapter`, which sums `Accumulator::size()` across groups to size its memory
reservation — making this under-report compound with group count.

## Design

Change `size()` to add the bins' allocated bytes instead of the `Vec` header size, using
DataFusion's `VecAllocExt::allocated_size()` helper (the pattern DataFusion itself uses in its
own `Accumulator`/`GroupsAccumulator::size()` implementations):

```rust
use datafusion::common::utils::proxy::VecAllocExt;

fn size(&self) -> usize {
    size_of_val(self) + self.bins.allocated_size()
}
```

`size_of_val(self)` already correctly accounts for the struct's own stack-resident fields
(including the 24-byte `Vec` header embedded in it), so only the heap allocation needs to be
added separately — no double-counting.

## Implementation Steps

1. In `rust/datafusion-extensions/src/histogram/accumulator.rs`, replace the `size()` body
   (lines 310-312) with the `allocated_size()`-based calculation shown above.
2. Add a unit test asserting `size()` grows with `nb_bins` (e.g. construct two
   `HistogramAccumulator`s via `HistogramAccumulator::new` with differing `nb_bins`, e.g. 10 and
   10,000, and assert the larger one's `size()` is larger by roughly `capacity_diff * 8` bytes).
   Add this to `rust/datafusion-extensions/tests/expand_histogram_tests.rs`, which already
   constructs `HistogramAccumulator` directly (via its `make_test_histogram` helper) and is the
   crate's established precedent for direct-construction accumulator tests, rather than inline in
   `accumulator.rs`.
3. Run `cargo fmt` and `cargo clippy --workspace -- -D warnings` in `rust/`.

## Files to Modify

- `rust/datafusion-extensions/src/histogram/accumulator.rs` — fix `size()`
- `rust/datafusion-extensions/tests/expand_histogram_tests.rs` — add the size-growth regression
  test

## Trade-offs

- Rather than a hand-written `self.bins.capacity() * size_of::<u64>()`, the fix uses
  `datafusion::common::utils::proxy::VecAllocExt::allocated_size()`, already reachable through the
  existing `datafusion` dependency. It computes the identical value but matches the idiomatic
  pattern DataFusion itself uses for `Accumulator`/`GroupsAccumulator::size()` implementations,
  so no one-off inline calculation or new abstraction is needed.

## Testing Strategy

- New unit test confirms `size()` scales with `nb_bins` (catches exactly the regression this issue
  describes — today's implementation returns the same size regardless of `nb_bins`).
- Existing tests in `histogram_runtime_bounds_tests.rs` and `expand_histogram_tests.rs` continue
  to pass, confirming no behavioral change to histogram construction/merging/evaluation — only
  `size()`'s accounting changes.
- `cargo test -p micromegas-datafusion-extensions` (or workspace-wide `cargo test`) to run the
  full suite.

## Open Questions

None — the issue specifies the exact fix and rationale; this plan follows it directly.
