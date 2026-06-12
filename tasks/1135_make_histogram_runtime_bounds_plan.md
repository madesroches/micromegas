# make_histogram Runtime Bounds Plan

## Overview

`make_histogram(lo, hi, bins, value)` currently requires its first three arguments to be compile-time `Literal` expressions. This plan removes that restriction so that scalar runtime expressions — including values derived from CTEs, subqueries, or CROSS JOINs — are accepted as bounds.

## Current State

The constraint lives entirely in `make_state()` in `rust/datafusion-extensions/src/histogram/histogram_udaf.rs` (lines 170–224). When DataFusion creates the accumulator for a query, it calls `make_state(AccumulatorArgs)`, which reads the argument expressions and calls `downcast_ref::<Literal>()` on each of the first three args. If any arg is not a `Literal` node in the physical plan — e.g. a `Column` reference from a CTE — the downcast returns `None` and the function returns `DataFusionError::Execution("Downcasting first argument to Literal")`.

The accumulator itself (`HistogramAccumulator` in `accumulator.rs`) already supports a "not yet configured" state:
- `new_non_configured()` (line 46) creates an accumulator with `start: None`, `end: None`, empty `bins`.
- `configure()` (line 61) lazily populates those fields from an existing `HistogramArray`.
- `update_batch_scalars()` (line 74) guards against an unconfigured accumulator and returns an error.

So the infrastructure for lazy initialization already exists. The missing piece is wiring the runtime scalar values (`values[0..=2]` in `update_batch`) into the accumulator when it is not yet configured.

**Latent bug to fix along the way:** `evaluate()` (accumulator.rs:180–193) already appends Arrow *field-level* nulls for `None` start/end (the unconfigured case, reachable today via `sum_histograms` over empty input), while `state_arrow_fields()` declares those fields `nullable = false`. And the merge path is not null-safe: `configure()` and `merge_histograms()` call `get_start`/`get_end`, whose `.value(idx)` silently returns `0.0` on a null slot, misconfiguring bounds to `[0.0, 0.0]`. This plan replaces that broken field-level-null encoding with a struct-level-null encoding (Design Change 3).

## Design

### Encoding decision: a NULL histogram is a null *struct row*, not a struct with null fields

When an accumulator finishes without ever being configured (empty partition, zero input rows), its output is a **null struct row**: `struct_builder.append(false)` with placeholder values in the child builders. Arrow permits a null struct slot over non-nullable children — the children hold placeholder values, not nulls, so child validity is unaffected.

Consequences, compared to making `start`/`end` nullable fields:

- `state_arrow_fields()` is **unchanged** — no public schema change for any consumer.
- `get_start()`/`get_end()` are **unchanged** — no Err-on-null control flow.
- A histogram row is either wholly null or fully valid; a "struct present but start is null, min is f64::MAX" half-state is not representable.
- SQL semantics are standard: an aggregate over zero rows yields `NULL`, and users can filter with `WHERE h IS NOT NULL`.
- Code that reads a row **must check struct validity first** (`is_null_at`, see Change 3); reading a null row's children returns placeholder values, same as Arrow defaults today.

### Change 1 — Graceful fallback in `make_state()`

`make_state()` in `histogram_udaf.rs` uses one simple rule:

- If **all three** args downcast to `Literal` **and** match the expected scalar pattern (`Float64(Some(_))` for start/end, `Int64(Some(_))` for nb_bins): build the accumulator eagerly via `new_non_configured()` + `configure_from_params(start, end, nb_bins)` (Change 2), propagating the validation `Err`. This keeps today's eager configuration but routes literal bounds through the same validation as runtime bounds — a literal `nb_bins = 0` or `start >= end` becomes a descriptive plan-time error instead of a panic or a garbage histogram.
- **Anything else** — a non-`Literal` expression, or a literal that doesn't match the pattern: return `HistogramAccumulator::new_non_configured()` and defer to runtime.

No multi-pass precedence analysis is needed. `create_udaf` declares the input types `(Float64, Float64, Int64, Float64)`, so the planner coerces or rejects mismatched literals before `make_state` ever runs — wrong-type literals mostly arrive constant-folded into correctly-typed literals or wrapped in cast expressions (which are non-`Literal` and take the runtime path). The one literal case that previously produced an eager error, a typed-`None` literal like `Float64(None)`, now takes the runtime path and is caught there by the explicit null-bound check in Change 2, with an equally clear error message.

While editing this function, fix the copy-pasted error messages at histogram_udaf.rs:199 and :215 ("arg 0 should be…" for args 1 and 2) if any literal validation messages survive the rewrite.

### Change 2 — Lazy configuration and validation in `update_batch`

In the 4-arg branch of `Accumulator::update_batch` (accumulator.rs:154), before calling `update_batch_scalars()`:

1. **Zero-row guard.** If `values[0].is_empty()`, return `Ok(())` — DataFusion may legally call `update_batch` with zero rows, and `.value(0)` on an empty array panics. The accumulator stays unconfigured until a non-empty batch arrives.
2. **Null-bound guard.** If any of `values[0..=2]` (start, end, *and* nb_bins) `.is_null(0)`, return `Err(DataFusionError::Execution(...))` with a message naming the argument. Without this, `Float64Array::value(0)` on a null slot silently yields `0.0` and `Int64Array::value(0)` yields `0` bins — which then panics in `update_batch_scalars` (`self.bins.len() - 1` underflow at accumulator.rs:95). This guard also covers `make_histogram(NULL, ...)` literal calls deferred by Change 1.
3. **Configure if needed.** If `self.start.is_none()`, downcast `values[0]`/`values[1]` to `Float64Array` and `values[2]` to `Int64Array`, read index 0 of each, and call `self.configure_from_params(start, end, nb_bins)`.
4. **Consistency check.** If already configured (whether eagerly from literals or from a previous batch), compare the batch's `value(0)` bounds and bin count against `self.start`/`self.end`/`self.bins.len()` and return `Err` on mismatch. Nothing else enforces that bounds are constant: `make_histogram(t.lo, t.hi, 10, v)` with per-row-varying `lo` would otherwise silently bin everything with the first row's bounds. The merge path already errors on incompatible bounds; the update path must be consistent. This per-batch first-row check is O(1); it does not catch intra-batch variation, which is acceptable — document the limitation in the udaf's doc comment. (Per-group bounds via GROUP BY still work: DataFusion feeds each group's accumulator only that group's rows.)

New method `HistogramAccumulator::configure_from_params(start: f64, end: f64, nb_bins: i64) -> Result<(), DataFusionError>`:
- Validate: `nb_bins >= 1`, `start.is_finite()`, `end.is_finite()`, `start < end`. Return a descriptive `Err` otherwise. Today a literal `nb_bins = 0` already panics; runtime bounds (e.g. `min(v), max(v)` over an unexpected distribution producing `start == end`) make these cases far easier to hit, so they must be real errors, not panics. Both the eager literal path (Change 1) and the runtime path call this method, so literal and runtime bounds get identical validation.
- Set `self.start = Some(start)`, `self.end = Some(end)`, `self.bins.resize(nb_bins as usize, 0)`.

### Change 3 — Null-struct-row support in serialization and merge

1. **`evaluate()`** (accumulator.rs:174) — when `self.start.is_none()` (unconfigured), append placeholder values to every child builder (`0.0` for floats, `0` for count, empty list for bins — `StructBuilder` requires every child to receive a value for each row) and finish the row with `struct_builder.append(false)`. When configured, behave as today with `append(true)`. The existing `append_null()` branches for start/end disappear: children are always non-null, matching `state_arrow_fields()` as declared.
2. **`HistogramArray::is_null_at()`** (histogram_udaf.rs) — new method: `pub fn is_null_at(&self, index: usize) -> bool { self.inner.is_null(index) }` (struct-level validity). `HistogramArray` exposes no null-check accessor today; every consumer change below depends on this helper.
3. **`configure()`** (accumulator.rs:61) — must not configure from element 0 blindly. `merge_batch` concatenates partial accumulator outputs in arbitrary order, so the array may have a leading null row (an empty partition) followed by valid rows. Scan for the first index where `!histo_array.is_null_at(idx)` and configure `self.start`/`self.end`/`self.bins` from **that same index** (start, end, *and* bins length must come from the same row — sizing bins from a null row's empty placeholder list while taking bounds from a later row would leave `self.bins` empty and panic in the merge loop at accumulator.rs:139–141). If every row is null, leave the accumulator unconfigured and return `Ok(())`.
4. **`merge_histograms()`** (accumulator.rs:102) — first statement of the loop body: `if histo_array.is_null_at(index_histo) { continue; }`, placed before the `get_start`/`get_end`/`self.start.unwrap()` accesses at the top of the loop. Null partial rows are ignored; valid rows at any index still merge. Do **not** add a blanket `if self.start.is_none() { return Ok(()); }` early-return — that would discard valid histograms whenever index 0 happened to be null. After `configure()` (step 3), `self.start.unwrap()` is only reached when at least one non-null row exists, and null rows `continue` before any getter call, so the unwraps cannot panic.

No change to `state_arrow_fields()` or `make_histogram_arrow_type()`. The struct *column* produced by the aggregate is nullable at the column level, which is normal for aggregate outputs; the struct's child fields remain non-nullable and the data honors that.

### Change 4 — Consumers handle NULL histogram rows

Every scalar UDF iterating a `HistogramArray` gets a row-level guard: `if histo_array.is_null_at(index) { result_builder.append_null(); continue; }` before reading any field. A NULL histogram in → a NULL result out, never a placeholder-derived garbage value.

- `quantile.rs` (`quantile_from_histogram`, loop at line 52)
- `accessors.rs` (`sum_from_histogram` loop at line 22, `count_from_histogram` loop at line 49)
- `variance.rs` (`variance_from_histogram`, loop at line 24)
- `expand.rs` (`expand_histogram_to_batch`, line 82) — add the guard at the top of the function, before the `get_start`/`get_end` reads at lines 86–87, returning `RecordBatch::new_empty(output_schema())` (mirroring the existing zero-bin early-return at lines 91–92). This covers both callers (`extract_histogram_from_struct` and the subquery path in `scan`); a query like `expand_histogram((SELECT make_histogram(...) FROM empty))` then yields zero rows instead of garbage bin centers.

`sum_histograms_udaf.rs` needs no edit: it already uses `new_non_configured()` and the shared merge path fixed in Change 3.

## Implementation Steps

1. **`accumulator.rs`** — add `configure_from_params(start, end, nb_bins)` with validation (Change 2).
2. **`accumulator.rs`** — update the 4-arg branch of `update_batch`: zero-row guard, null-bound guard, lazy configure, per-batch consistency check (Change 2).
3. **`histogram_udaf.rs`** — add `HistogramArray::is_null_at()` (Change 3.2).
4. **`accumulator.rs`** — update `evaluate()` to emit a null struct row with placeholder children when unconfigured (Change 3.1).
5. **`accumulator.rs`** — update `configure()` to scan for the first non-null row and read start/end/bins from that same index (Change 3.3).
6. **`accumulator.rs`** — update `merge_histograms()` to skip null rows as the first statement of the loop (Change 3.4).
7. **`histogram_udaf.rs`** — rewrite `make_state()`: all-valid-literals → `new_non_configured()` + `configure_from_params()` (validated eager path), anything else → plain `new_non_configured()` (Change 1).
8. **`quantile.rs`, `accessors.rs`, `variance.rs`, `expand.rs`** — add null-row guards (Change 4).
9. **`tests/`** — add `histogram_runtime_bounds_tests.rs` (see Testing Strategy).

## Files to Modify

- `rust/datafusion-extensions/src/histogram/accumulator.rs` — `configure_from_params` + `update_batch` guards; null-struct-row `evaluate()`; first-non-null `configure()`; null-skipping `merge_histograms()`
- `rust/datafusion-extensions/src/histogram/histogram_udaf.rs` — `is_null_at()` helper; relaxed `make_state()`
- `rust/datafusion-extensions/src/histogram/quantile.rs` — null-row guard
- `rust/datafusion-extensions/src/histogram/accessors.rs` — null-row guards (sum, count)
- `rust/datafusion-extensions/src/histogram/variance.rs` — null-row guard
- `rust/datafusion-extensions/src/histogram/expand.rs` — null-row guard in `expand_histogram_to_batch`
- `rust/datafusion-extensions/tests/histogram_runtime_bounds_tests.rs` — new test file

## Trade-offs

**Chosen approach — lazy accumulator configuration + struct-level null encoding:**
Minimal change, consistent with the existing `new_non_configured()`/`configure()` pattern. No public schema change, no API change to callers, and it fixes the existing field-null/non-nullable-schema mismatch rather than entrenching it.

**Rejected — field-level nullable `start`/`end`:**
Making the two fields nullable in `state_arrow_fields()` changes the public histogram output type for every consumer, requires `get_start`/`get_end` to grow Err-on-null semantics that call sites must interpret as "skip, not abort", and exposes half-valid rows (struct present, `start` null, `min`/`max` still the `f64::MAX`/`f64::MIN` sentinels). Struct-level null gets the same expressiveness with a stronger invariant and far fewer touch points.

**Rejected — DataFusion optimizer rule:**
A custom optimizer rule could fold scalar subqueries into `Literal` nodes before the UDAF sees them. Far more code for a benefit that only helps this one UDAF, and it cannot handle bounds that vary per group.

**Rejected — newer `AggregateUDFImpl` trait:**
DataFusion 53 supports `impl AggregateUDFImpl`, which gives more hooks into planning. A larger refactor that is not needed to solve this issue.

## Testing Strategy

Integration tests (async, `SessionContext` with all histogram extensions registered) in `histogram_runtime_bounds_tests.rs`:

1. **Runtime bounds happy path** — CTE computes `lo`/`hi` via `min(v)`/`max(v)`, CROSS JOIN feeds them to `make_histogram`; assert a non-null histogram with the expected `start`/`end`/bin count and total count.
2. **Zero input rows** — `make_histogram` (runtime-bounds form) over an empty table yields SQL `NULL`; `sum_histograms` over empty input likewise.
3. **NULL histogram through consumers** — `quantile_from_histogram`, `sum_from_histogram`, `count_from_histogram`, `variance_from_histogram` over a NULL histogram return NULL; `expand_histogram` over it returns zero rows.
4. **Invalid bounds** — NULL bound value, `nb_bins = 0`, and `start >= end` each produce a descriptive error, not a panic — in both the literal spelling (caught at plan time by `make_state()`) and the runtime-expression spelling (caught in `update_batch`).
5. **Inconsistent bounds** — a bounds column that varies across batches produces the consistency-check error.
6. **Literal bounds happy path** — `make_histogram(0.0, 100.0, 10, value)` with literal bounds via SQL produces the same histogram as before. This package currently has **no** SQL-level coverage of the literal path (`expand_histogram_tests.rs` constructs `HistogramAccumulator` directly and never calls `make_state()`; the literal-bounds SQL tests in `micromegas-analytics` are `#[ignore]`d behind live services, and `datafusion-wasm` is excluded from the workspace), so this test is the regression guard for the `make_state()` rewrite. Run `cargo test -p micromegas-datafusion-extensions` for the rest of the package.
