# make_histogram Runtime Bounds Plan

## Overview

`make_histogram(lo, hi, bins, value)` currently requires its first three arguments to be compile-time `Literal` expressions. This plan removes that restriction so that scalar runtime expressions — including values derived from CTEs, subqueries, or CROSS JOINs — are accepted as bounds.

## Current State

The constraint lives entirely in `make_state()` in `rust/datafusion-extensions/src/histogram/histogram_udaf.rs` (lines 170–224). When DataFusion creates the accumulator for a query, it calls `make_state(AccumulatorArgs)`, which reads the argument expressions and calls `downcast_ref::<Literal>()` on each of the first three args. If any arg is not a `Literal` node in the logical plan — e.g. a `Column` reference from a CTE — the downcast returns `None` and the function returns `DataFusionError::Execution("Downcasting first argument to Literal")`.

The accumulator itself (`HistogramAccumulator` in `accumulator.rs`) already supports a "not yet configured" state:
- `new_non_configured()` (line 53) creates an accumulator with `start: None`, `end: None`, empty `bins`.
- `configure()` (line 66) lazily populates those fields from an existing `HistogramArray`.
- `update_batch_scalars()` (line 88) guards against an unconfigured accumulator and returns an error.

So the infrastructure for lazy initialization already exists. The missing piece is wiring the runtime scalar values (`values[0..2]` in `update_batch`) into the accumulator when it is not yet configured.

## Design

### Change 1 — Graceful fallback in `make_state()`

`make_state()` in `histogram_udaf.rs` should be changed to:
1. Attempt the existing `Literal` downcast for all three args.
2. If all succeed, return `HistogramAccumulator::new(start, end, nb_bins)` as today.
3. If any downcast fails, return `HistogramAccumulator::new_non_configured()`.

This means the accumulator is allowed to be born without knowing its bounds. The bounds will be supplied in `update_batch`.

### Change 2 — Lazy configuration in `update_batch`

In the 4-arg branch of `Accumulator::update_batch` in `accumulator.rs` (line ~148), before calling `update_batch_scalars()`, add a configuration step:

```
if not configured {
    start  = values[0] as Float64Array, take value(0)
    end    = values[1] as Float64Array, take value(0)
    bins   = values[2] as Int64Array,   take value(0) as usize
    self.configure_from_params(start, end, bins)
}
```

Add a new method `HistogramAccumulator::configure_from_params(start: f64, end: f64, nb_bins: usize)` that sets the three fields and resizes `self.bins`. This mirrors what `new()` does but works on an already-allocated accumulator.

`values[0..2]` are constant across all rows of a batch (either they are broadcast literals or uniform columns from a scalar CROSS JOIN). Taking `value(0)` is correct and sufficient; no need to validate every row. Guard against the zero-row case: if `values[0].is_empty()`, return `Ok(())` immediately without calling `configure_from_params`, leaving the accumulator unconfigured until a non-empty batch arrives. DataFusion may legally call `update_batch` with zero rows, and calling `.value(0)` on an empty array panics.

### Change 3 — Fix nullable mismatch for `start` / `end` in accumulator state

`state_arrow_fields()` currently declares `start` and `end` with `nullable = false`. However, `evaluate()` already appends Arrow nulls for `None` start/end (the unconfigured case introduced by `new_non_configured()`). This creates a schema/data mismatch: Arrow rejects or silently misrepresents nulls in a non-nullable field.

More critically, the `merge_batch` path is **not** already safe: it calls `configure()` → `get_start(0)` / `get_end(0)`, which calls `.value(0)` on the `Float64Array` without a null check. When the array slot is null (an unconfigured accumulator was serialized), `.value(0)` returns `0.0` (Arrow's default for an unset float), silently misconfiguring the merged accumulator's bounds to `[0.0, 0.0]` instead of propagating the unconfigured state.

Fix both sides of the mismatch:

1. **`state_arrow_fields()`** — change the `start` and `end` `Field` declarations to `nullable = true`.
2. **`get_start()` / `get_end()` in `histogram_udaf.rs`** — guard against the null slot before calling `.value()`. Return `Err(DataFusionError::Execution(...))` when the slot is null, keeping the existing `Result<f64, DataFusionError>` return type so all `?` operators at call sites continue to compile unchanged.
3. **`configure()` in `accumulator.rs`** — handle the `Err` return from `get_start` / `get_end` by keeping `self.start` and `self.end` as `None`, leaving the accumulator in the unconfigured state rather than writing garbage bounds.

### No changes needed

- `make_histogram_arrow_type()` — the return type is `DataType::Struct(Fields::from(state_arrow_fields()))`. The `bins` field is `DataType::List(List<UInt64>)`, which is variable-length, so the Arrow type does not depend on the number of bins and needs no change.
- All downstream UDFs (`sum_histograms`, `quantile_from_histogram`, accessors, `expand`) — they operate on the fully-evaluated histogram struct, unaffected.

## Implementation Steps

1. **`accumulator.rs`** — add `configure_from_params(start: f64, end: f64, nb_bins: usize)`:
   - Set `self.start = Some(start)`, `self.end = Some(end)`.
   - `self.bins.resize(nb_bins, 0)`.

2. **`accumulator.rs`** — update 4-arg branch of `update_batch`:
   - After extracting `values[3]` as `Float64Array`, add a guard: if `self.start.is_none()`, downcast `values[0]` to `Float64Array`, `values[1]` to `Float64Array`, `values[2]` to `Int64Array`, read index 0 of each, call `self.configure_from_params(...)`.
   - Before reading `.value(0)`, add an early return when `values[0].is_empty()` (DataFusion may call `update_batch` with a zero-row batch). In that case, leave the accumulator unconfigured and return `Ok(())`.

3. **`histogram_udaf.rs`** — update `make_state()`:
   - Wrap each `downcast_ref::<Literal>()` block in a helper or use `if let` chains.
   - If all three succeed, construct `HistogramAccumulator::new(start, end, nb_bins)` as before.
   - Otherwise, construct `HistogramAccumulator::new_non_configured()`.

4. **`histogram_udaf.rs`** — update `get_start()` and `get_end()`:
   - Before calling `.value(idx)` on the `Float64Array`, check `.is_null(idx)`.
   - Return `Err(DataFusionError::Execution("histogram slot is null"))` when the slot is null, preserving the existing `Result<f64, DataFusionError>` return type.

5. **`accumulator.rs`** — update `configure()`:
   - Match on the `Err` return from `get_start` / `get_end`; when either returns an error (null slot), return `Ok(())` immediately, leaving `self.start` / `self.end` as `None` so the accumulator stays in the unconfigured state. Do **not** use `?` here — `?` propagates the error to the caller rather than absorbing it, which would not leave the accumulator unconfigured.
   - The inner-loop `merge_histograms()` call sites (lines 108 and 114) use `get_start(index_histo)?` and `get_end(index_histo)?`. Because `get_start`/`get_end` remain `Result`-returning, `?` continues to work there without changes.
   - Also update `merge_histograms()`: after calling `self.configure(histo_array)?`, check `self.start.is_some()` before reaching `self.start.unwrap()` (line 109) and `self.end.unwrap()` (line 115). If `self.start` is still `None` (the first histogram slot was null), return early with `Ok(())` to skip the merge for that batch.

6. **`accumulator.rs`** — update `state_arrow_fields()`:
   - Change the `start` and `end` `Field` entries to `nullable = true`.

7. **`tests/`** — add `histogram_runtime_bounds_tests.rs`:
   - Register all extensions on a `SessionContext`.
   - Create an in-memory table with float values.
   - Execute a query using a CTE to compute `lo`/`hi` via `percentile_cont` or `min`/`max`, then CROSS JOIN to use them as bounds in `make_histogram`.
   - Assert the result is a non-null histogram struct with correct bin count.

## Files to Modify

- `rust/datafusion-extensions/src/histogram/accumulator.rs` — new method + update `update_batch`; fix `configure()` null propagation; change `start`/`end` fields to `nullable = true` in `state_arrow_fields()`
- `rust/datafusion-extensions/src/histogram/histogram_udaf.rs` — add null guards to `get_start()` / `get_end()`; relax `make_state()`
- `rust/datafusion-extensions/tests/histogram_runtime_bounds_tests.rs` — new test file

## Trade-offs

**Chosen approach — lazy accumulator configuration:**
Minimal change, consistent with the existing `new_non_configured()` / `configure()` pattern already used for the merge path. No API changes to callers.

**Alternative — DataFusion optimizer rule:**
A custom optimizer rule could fold scalar subqueries into `Literal` nodes before the UDAF sees them. This would require implementing `OptimizerRule` and registering it — far more code for a benefit that only helps this one UDAF.

**Alternative — newer `AggregateUDFImpl` trait:**
DataFusion 53 supports `impl AggregateUDFImpl` which gives more hooks into planning. Migrating to it would be a larger refactor and is not needed to solve this issue.

## Testing Strategy

1. Add a DataFusion integration test (async, uses `SessionContext`) that:
   - Registers `make_histogram` (and optionally `sum_histograms`).
   - Executes a query where `lo` and `hi` come from a CTE (e.g., `SELECT min(v), max(v) FROM t`) cross-joined with the data table.
   - Verifies the resulting histogram struct is non-null and `start`/`end` match the CTE values.
2. Confirm the existing literal-bounds tests still pass (`cargo test -p micromegas-datafusion-extensions`).
