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

`make_state()` in `histogram_udaf.rs` should distinguish three cases:
1. **All literal and valid** — all three args downcast to `Literal` **and** match the expected scalar pattern (`Float64(Some(_))` for start/end, `Int64(Some(_))` for nb_bins). Return `HistogramAccumulator::new(start, end, nb_bins)` as today.
2. **Non-literal runtime expression** — at least one arg is *not* a `Literal` node (e.g. a `Column` reference from a CTE/CROSS JOIN). Fall back to `HistogramAccumulator::new_non_configured()`; the bounds will be supplied at runtime in `update_batch`.
3. **Wrong-type or `None` literal** — an arg *is* a `Literal` but its `ScalarValue` is not the expected `Float64(Some)` / `Int64(Some)` (e.g. a string literal, a `Float64(None)`, or any typed `None`). Keep the existing explicit `return Err(...)` for this case: a literal bound of the wrong type is a genuine user error and must fail early with a clear message rather than deferring to a generic runtime failure or, worse, silently reading a null literal as `0.0` in `update_batch`.

The key distinction is *literal vs. non-literal*, not *valid vs. invalid*. Only a non-`Literal` expression defers to `new_non_configured()`; a present-but-wrong literal still errors.

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

More critically, the `merge_batch` path is **not** already safe: `configure()` and the `merge_histograms` loop call `get_start` / `get_end`, which call `.value(idx)` on the `Float64Array` without a null check. When a slot is null (an unconfigured accumulator was serialized — e.g. an empty partition), `.value(idx)` returns `0.0` (Arrow's default for an unset float), silently misconfiguring the bounds to `[0.0, 0.0]`. Because `merge_batch` concatenates partial outputs in arbitrary order, the null can land at any index, including before valid rows.

Fix both sides of the mismatch:

1. **`state_arrow_fields()`** — change the `start` and `end` `Field` declarations to `nullable = true`.
2. **`get_start()` / `get_end()` in `histogram_udaf.rs`** — guard against the null slot before calling `.value()`. Return `Err(DataFusionError::Execution(...))` when the slot is null, keeping the existing `Result<f64, DataFusionError>` return type. Call sites must treat this `Err` as "skip this row", not "abort", so they no longer blindly `?`-propagate (see Change 3 step / Implementation Step 5).
3. **`configure()` in `accumulator.rs`** — do not infer the configured state from slot 0. The serialized array can have a leading null (an empty/unconfigured partition concatenated by `merge_batch`) followed by valid rows. Scan for the first non-null start/end slot and configure from it; only leave `self.start` / `self.end` as `None` when **every** slot is null. The `merge_histograms` loop then skips individual null rows (`continue`) rather than `?`-propagating, so valid histograms at index ≥ 1 are merged even when index 0 is null.

### Output type and downstream consumers

`make_histogram_arrow_type()` builds `DataType::Struct(Fields::from(state_arrow_fields()))` and `create_udaf` uses it as **both** the UDAF's final return type and its intermediate state type. Marking `start` / `end` nullable in `state_arrow_fields()` therefore also makes them nullable in the public histogram output struct seen by all consumers — this is not merely an intermediate-state schema fix.

Because every consumer (`sum_histograms_udaf.rs`, `accessors.rs`, `quantile.rs`, `variance.rs`, `expand`) reads the histogram type through the shared `make_histogram_arrow_type()` / `state_arrow_fields()`, there is no hard schema mismatch: the producer and all consumers move to the same nullable definition together. No consumer code needs to change, but the change is intentionally a public-output-type change (start/end become nullable everywhere), not a state-only tweak.

`make_histogram_arrow_type()` itself needs no edit beyond the `state_arrow_fields()` change it composes: the `bins` field is `DataType::List(List<UInt64>)`, variable-length, so the Arrow type does not depend on the number of bins.

## Implementation Steps

1. **`accumulator.rs`** — add `configure_from_params(start: f64, end: f64, nb_bins: usize)`:
   - Set `self.start = Some(start)`, `self.end = Some(end)`.
   - `self.bins.resize(nb_bins, 0)`.

2. **`accumulator.rs`** — update 4-arg branch of `update_batch`:
   - After extracting `values[3]` as `Float64Array`, add a guard: if `self.start.is_none()`, downcast `values[0]` to `Float64Array`, `values[1]` to `Float64Array`, `values[2]` to `Int64Array`, read index 0 of each, call `self.configure_from_params(...)`.
   - Before reading `.value(0)`, add an early return when `values[0].is_empty()` (DataFusion may call `update_batch` with a zero-row batch). In that case, leave the accumulator unconfigured and return `Ok(())`.

3. **`histogram_udaf.rs`** — update `make_state()`, keeping the three cases distinct:
   - For each of the three args, first try `downcast_ref::<Literal>()`.
   - **If the downcast fails** (non-`Literal` expression — the runtime path), construct `HistogramAccumulator::new_non_configured()` and return it. Do this as soon as any one of the three args is non-literal.
   - **If all three downcast to `Literal`**, match each on the expected `ScalarValue` pattern (`Float64(Some(_))` for start/end, `Int64(Some(_))` for nb_bins):
     - When all match, construct `HistogramAccumulator::new(start, end, nb_bins)`.
     - When a literal is present but the scalar is the wrong type or a typed `None` (e.g. string literal, `Float64(None)`), keep the existing `return Err(...)` with a descriptive message. Do **not** fall through to `new_non_configured()` here — a wrong-type literal is a user error and must fail early (otherwise a `Float64(None)` literal would later be read as `0.0` by `value(0)` in Change 2).
   - Net effect: the `Literal`-downcast-failure path is the only one that now diverts to `new_non_configured()`; the wrong-type-literal `return Err(...)` is retained, not removed.

4. **`histogram_udaf.rs`** — update `get_start()` and `get_end()`:
   - Before calling `.value(idx)` on the `Float64Array`, check `.is_null(idx)`.
   - Return `Err(DataFusionError::Execution("histogram slot is null"))` when the slot is null, preserving the existing `Result<f64, DataFusionError>` return type.

5. **`accumulator.rs`** — update `configure()` and `merge_histograms()` so leading nulls do not discard valid rows:
   - **`configure()`** must not decide configured/unconfigured from element 0 alone. `merge_batch` concatenates partial accumulator outputs in arbitrary order, so the array may have a leading null (an empty/unconfigured partition) followed by valid rows. Scan for the first **non-null** start/end slot and configure `self.start` / `self.end` from that index; only leave the accumulator unconfigured if **every** slot is null. Use `is_null(idx)` to find the first valid slot rather than blindly reading index 0.
   - **`merge_histograms()` loop body** — for each `index_histo`, skip rows whose start/end slot is null. Replace the `get_start(index_histo)?` / `get_end(index_histo)?` calls with a null check (`histo_array.is_null_at(index_histo)` or matching `Err` from `get_start`/`get_end`) that `continue`s to the next row instead of `?`-propagating. This way a null partial row (an unconfigured partition) is ignored, while valid rows at index ≥ 1 are still merged. Do **not** insert a blanket `if self.start.is_none() { return Ok(()); }` early-return: that would discard every valid histogram whenever index 0 happened to be null.
   - Once a non-null row has configured `self.start` / `self.end`, the per-row `self.start.unwrap()` / `self.end.unwrap()` inside the loop are only reached for non-null rows (null rows `continue` before that point), so they no longer panic.

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
