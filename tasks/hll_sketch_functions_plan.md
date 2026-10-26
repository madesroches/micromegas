# HyperLogLog Sketch Functions Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1643

## Overview

Add three SQL functions — `make_hll(value)`, `merge_hll(sketch)` and `hll_cardinality(sketch)` —
so distinct counts (DAU/WAU/MAU, unique players per platform) can be pre-aggregated in
materialized views and merged losslessly across partitions long after the raw identifiers have
expired. They mirror `make_histogram` / `sum_histograms` / `count_from_histogram`. Sketches are
serialized in the Apache DataSketches HLL format so they remain mergeable by Spark/Databricks.

## Current State

- Aggregate and scalar functions that also compile to wasm32 live in
  `rust/datafusion-extensions`. The histogram family is the template:
  `src/histogram/histogram_udaf.rs` (`make_histogram`), `src/histogram/sum_histograms_udaf.rs`
  (merge via `create_udaf`, reusing the accumulator for `update_batch` and `merge_batch`), and
  `src/histogram/accessors.rs` (`count_from_histogram` via `create_udf`).
- Everything is registered in `register_extension_udfs` (`src/lib.rs`), which is shared by the
  server and `rust/datafusion-wasm`.
- There is no distinct-count sketch today; `count(DISTINCT)` / `approx_distinct` need the raw data.
- `datasketches` 0.3.0 (Apache DataSketches Rust, Apache-2.0, **no dependencies**, so wasm32-safe)
  provides `hll::{HllSketch, HllUnion, HllType}` with `serialize()` / `deserialize()` in the
  standard DataSketches binary format, plus `Coupon::from_hash` and `hash_value` wrappers.
  It is not yet in `rust/Cargo.toml`.
- DataFusion's internal HLL (`approx_distinct`) exposes only the estimate, not a mergeable
  serialized state, so it cannot be reused.

## Design

### Dependency and format

Add `datasketches = { version = "0.3", features = ["hll"] }` to the workspace `rust/Cargo.toml` and
reference it from `rust/datafusion-extensions/Cargo.toml` (the crate has `default = []`; `hll` is
feature-gated). `rust/datafusion-wasm/Cargo.lock` is a separate lockfile and needs the new crate.
Sketches are `DataType::Binary` values holding `HllSketch::serialize()` output, built as
`HllType::Hll4` (smallest: ~8 KB at lg_k=14 when dense, far smaller in list/set mode for low
cardinalities). The serialized form is DataSketches-compatible; the crate's
`tests/hll_serialization_test.rs` has Java/C++ compatibility tests for Hll4/6/8, so Hll4 interop is
covered.

### Precision

Fixed `lg_k = 14` (~0.8% standard error) as a shared constant. `make_hll(value)` takes a single
argument. An optional `lg_k` argument is deferred: it adds a literal-parameter code path like
`make_histogram`'s for no pressing need. `merge_hll` accepts sketches of any `lg_k`;
`HllUnion` downsamples to the lowest `lg_k` seen (its documented rule), so sketches produced by
Spark at another precision still merge. `HllUnion::new(14)` caps the result at 14.

### `make_hll(value)` — aggregate

- Accepts `Utf8`, `LargeUtf8`, `Utf8View`, `Int8..Int64`, `UInt8..UInt64`, `Float32/64`, `Binary`,
  `Dictionary` of those; a small set of signatures registered via `Signature::one_of`/coercible
  rather than a single fixed type. Implement as `AggregateUDFImpl` (as `create_udaf` only supports
  fixed argument types).
- Hashing follows the DataSketches cross-language conventions via `datasketches::hash_value`:
  strings as UTF-8 raw bytes, integers via `sign_extend` to 64 bits, floats via `canonical_float`,
  so a Spark-built sketch over the same column agrees with ours.
- Accumulator holds one `HllSketch`; `update_batch` skips nulls; `state()` is a single `Binary`
  scalar; `merge_batch` unions incoming serialized states; `evaluate()` returns the serialized
  sketch, or NULL when the group saw no non-null values (see Decisions).
- Memory accounting via `size()`.

### `merge_hll(sketch)` — aggregate

Input and output `Binary`. Accumulator holds an `HllUnion::new(14)`. `update_batch` deserializes
each non-null input and `union.update(&sketch)`; `state()`/`merge_batch` exchange the serialized
unioned sketch. A corrupt or non-HLL payload yields `DataFusionError::Execution` naming the
function, not a panic. Output: `union.to_sketch(HllType::Hll4).serialize()`, or NULL when every
input was null/empty.

### `hll_cardinality(sketch)` — scalar

`Binary → UInt64`: deserialize, `estimate().round() as u64`. NULL in → NULL out. Invalid bytes →
execution error.

### Shared module

New `src/hll/` module (`mod.rs`, `make_hll_udaf.rs`, `merge_hll_udaf.rs`, `cardinality.rs`) with a
single `LG_K` constant and one helper pair (`serialize_sketch`, `deserialize_sketch` with error
mapping) used by all three functions. Both aggregates share one accumulator type parameterized
over how an input row is added (hash a value vs. union a sketch) to avoid duplicating
`state`/`merge_batch`/`size`.

### Hash keying (membership-inference mitigation)

Out of scope for the first version: the crate hashes with a fixed seed, and a secret-keyed hash
needs a stable key stored in view definitions. Documented as a trade-off, with the guidance to
materialize sketches only for large groups. `Coupon::from_hash` leaves room to add an optional
key argument later without changing the sketch format.

### WASM

`datasketches` has no dependencies, so the functions register through `register_extension_udfs`
and are available in `datafusion-wasm` automatically; verify the wasm build still compiles.

## Implementation Steps

1. Add `datasketches = { version = "0.3", features = ["hll"] }` to `rust/Cargo.toml` (workspace deps)
   and reference it from `rust/datafusion-extensions/Cargo.toml`; add HLL to that crate's
   description/keywords; refresh `rust/datafusion-wasm/Cargo.lock` (separate lockfile).
2. Create `rust/datafusion-extensions/src/hll/` with the shared helpers and `LG_K`.
3. Implement the `make_hll` `AggregateUDFImpl` and accumulator.
4. Implement the `merge_hll` UDAF reusing the shared accumulator.
5. Implement `hll_cardinality` scalar UDF.
6. Declare `pub mod hll;` and register the three functions in `register_extension_udfs`
   (`src/lib.rs`).
7. Unit tests (below).
8. Docs: `mkdocs/docs/query-guide/functions-reference.md`, extension crate `README.md`,
   `doc/how_to_query/README.md`, `claude-plugin/skills/micromegas-query/SKILL.md`, `CHANGELOG.md`.

## Files to Modify

- `rust/Cargo.toml`, `rust/datafusion-extensions/Cargo.toml` (dependency, plus HLL in description/keywords)
- `rust/datafusion-wasm/Cargo.lock`
- `rust/datafusion-extensions/src/lib.rs`
- `rust/datafusion-extensions/tests/hll_tests.rs` (new)
- `rust/datafusion-extensions/src/hll/{mod,make_hll_udaf,merge_hll_udaf,cardinality}.rs` (new)
- `rust/datafusion-extensions/README.md`
- `mkdocs/docs/query-guide/functions-reference.md`
- `doc/how_to_query/README.md`
- `claude-plugin/skills/micromegas-query/SKILL.md`
- `CHANGELOG.md`

## Trade-offs

- **Use `datasketches` crate vs. hand-rolled serializer.** The crate already implements the
  DataSketches HLL format, unions and bounds; writing our own risks subtle incompatibility.

## Decisions

- Empty input (no non-null values) yields NULL rather than an empty serialized sketch, consistent
  with other aggregates; `hll_cardinality(NULL)` is NULL, and `COALESCE(..., 0)` covers display.
- Keyed hashing is deferred (see Design); documented rather than half-implemented.
- `lg_k` is fixed at 14 and not a `make_hll` argument; merge handles mixed precisions.
- DataFusion's `approx_distinct` HLL is not reused: it exposes no serialized mergeable state.

## Documentation

- `functions-reference.md`: new "HyperLogLog Functions" section with signatures, return types, the
  accuracy (~0.8%), the DataSketches format/Spark compatibility note, the privacy trade-off, and a
  daily-uniques materialized view example (extract query with `make_hll`, merge query with
  `merge_hll`, and a MAU query with `hll_cardinality(merge_hll(...))`).
- Intersections are not supported (inclusion–exclusion is inaccurate); note this and point to
  cohort-property aggregation.

## Testing Strategy

No-DB tests in `rust/datafusion-extensions/tests/hll_tests.rs`:
- Accuracy: 100k distinct strings within ~3% (about 3.5 sigma of the 0.8% standard error); small
  exact-ish counts (list mode).
- Merge equals sketch of the union: merge of sketches over two overlapping ranges ≈ sketch over
  the union, exactly equal serialized estimate.
- Duplicates do not inflate; nulls ignored; all-null/empty group → NULL.
- Types: Utf8, Int64, Float64, Utf8View accepted.
- Mixed `lg_k` merge: a sketch built at lg_k=12 via the crate merges with an lg_k=14 one.
- Invalid bytes produce an error from `merge_hll` and `hll_cardinality`.
- Two-phase aggregation: run through a `SessionContext` with multiple partitions so `state` /
  `merge_batch` are exercised (in-memory tables, no external DB).
- A `SessionContext` test of the materialized-view query shape: grouped `make_hll`, then
  `hll_cardinality(merge_hll(...))` over the grouped result.
- Binary and Utf8 hash agreement: `make_hll` over the same bytes as `Binary` and as `Utf8` yields
  identical sketches.
- Dictionary-encoded and Utf8View inputs produce the same estimate as plain Utf8.
- wasm: covered by the CI wasm job (`./build/rust_ci.py wasm`).
