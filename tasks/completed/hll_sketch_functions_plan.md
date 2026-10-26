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
- `datasketches` 0.5.0 (Apache DataSketches Rust, Apache-2.0, **no dependencies**, so wasm32-safe)
  provides `hll::{HllSketch, HllUnion, HllType}` (constructors return `Result`) with `serialize()` / `deserialize()` in the
  standard DataSketches binary format, plus `Coupon::from_hash` and `hash::value` wrappers.
  It is not yet in `rust/Cargo.toml`.
- DataFusion's internal HLL (`approx_distinct`) exposes only the estimate, not a mergeable
  serialized state, so it cannot be reused.

## Design

### Dependency and format

Add `datasketches = { version = "0.5", features = ["hll"] }` to the workspace `rust/Cargo.toml` and
reference it from `rust/datafusion-extensions/Cargo.toml` (`hll` is a feature of `datasketches`).
Sketches are `DataType::Binary` values holding `HllSketch::serialize()` output, built as
`HllType::Hll4` (smallest: ~8 KB at lg_k=14 when dense, far smaller in list/set mode for low
cardinalities). The serialized form is DataSketches-compatible; the crate's
`tests/hll_serialization_test.rs` has Java/C++ compatibility tests for Hll4/6/8, so Hll4 interop is
covered.

### `make_hll(value)` — aggregate

- Single argument. Types: `Utf8`, `LargeUtf8`, `Utf8View`, `Int8..Int64`, `UInt8..UInt64`,
  `Float32/64`, `Binary`, and `Dictionary` of those (unpacked to its value type).
- Implemented as `AggregateUDFImpl` (as `create_udaf` only supports fixed argument types) with
  `Signature::any(1, ...)` and explicit `DataType` dispatch in `update_batch` (as `jsonb/cast.rs`
  does), rejecting unsupported types with a plan error. `state_fields` is one nullable `Binary`;
  `return_type` is `Binary`.
- `HllSketch::update` hashes via std `Hash` (MurmurHash3; a plain `&str` appends 0xff and a
  `&[u8]` adds a length prefix), which does not match other DataSketches implementations. Each
  value is therefore wrapped explicitly with `datasketches::hash::value` before `update_value`:

  | DataType | Hashing |
  |---|---|
  | `Utf8`, `LargeUtf8`, `Utf8View` | `raw_bytes::from_str` |
  | `Binary` | `raw_bytes::from_slice` |
  | `Int8`, `Int16`, `Int32` | `sign_extend::from_i8/i16/i32` |
  | `UInt8`, `UInt16`, `UInt32` | `sign_extend::from_u8/u16/u32` |
  | `Int64`, `UInt64` | hashed directly (`UInt64` via wrapping cast to `i64`) |
  | `Float32`, `Float64` | `canonical_float::from_f32/f64` |
  | `Dictionary` | by its value type |

- Accumulator shape shared with `merge_hll`: it holds an `HllUnion::new(LG_K)` and a fn that adds
  one row to the union. `make_hll` calls `union.update_value` with the wrapped value; nulls and
  empty strings/bytes are skipped. `state()` and `evaluate()` both emit
  `union.to_sketch(HllType::Hll4).serialize()` (single `Binary` scalar; `evaluate()` is NULL when
  the group saw no values, see Decisions); `merge_batch` deserializes each state and calls
  `union.update`. `size()` is `size_of::<Self>()` adjusted by `HllUnion::estimated_size()`.

### `merge_hll(sketch)` — aggregate

Input and output `Binary`. Same accumulator as `make_hll`, with the add-row fn deserializing each
non-null input and calling `union.update(&sketch)`; `state()`/`merge_batch` exchange the
serialized unioned sketch. A corrupt or non-HLL payload yields `DataFusionError::Execution`
naming the function, not a panic. Output: `union.to_sketch(HllType::Hll4).serialize()`, or NULL
when every input was null/empty. Sketches of any `lg_k` are accepted (e.g. Spark at another
precision): `HllUnion::new(14)` caps the result at 14, and a lower-`lg_k` input downsamples the
result to that `lg_k` (array-mode sketches only).

### `hll_cardinality(sketch)` — scalar

`Binary → UInt64`: deserialize, `estimate().round() as u64`. NULL in → NULL out. Invalid bytes →
execution error.

### Shared module

New `src/hll/` module (`mod.rs`, `make_hll_udaf.rs`, `merge_hll_udaf.rs`, `cardinality.rs`) with a
single `LG_K` constant, one helper pair (`serialize_sketch`, `deserialize_sketch` with error
mapping) used by all three functions, and the union accumulator described above, parameterized
over the add-row fn.

### WASM

`datasketches` has no dependencies, so the functions register through `register_extension_udfs`
and are available in `datafusion-wasm` automatically; verify the wasm build still compiles.

## Implementation Steps

1. Add `datasketches = { version = "0.5", features = ["hll"] }` to `rust/Cargo.toml` (workspace deps)
   and reference it from `rust/datafusion-extensions/Cargo.toml`; add HLL to that crate's
   description/keywords; refresh `rust/Cargo.lock` and `rust/datafusion-wasm/Cargo.lock` (separate lockfile).
2. Create `rust/datafusion-extensions/src/hll/` with the shared helpers and `LG_K`.
3. Implement the `make_hll` `AggregateUDFImpl` and accumulator.
4. Implement the `merge_hll` UDAF reusing the shared accumulator.
5. Implement `hll_cardinality` scalar UDF.
6. Declare `pub mod hll;` and register the three functions in `register_extension_udfs`
   (`src/lib.rs`).
7. Unit tests (below).
8. Docs: `mkdocs/docs/query-guide/functions-reference.md`, extension crate `README.md`,
   `doc/how_to_query/README.md`, `mkdocs/docs/query-guide/quick-start.md` (function list), `claude-plugin/skills/micromegas-query/SKILL.md`, `CHANGELOG.md`.

## Files to Modify

- `rust/Cargo.toml`, `rust/datafusion-extensions/Cargo.toml` (dependency, plus HLL in description/keywords)
- `rust/Cargo.lock`, `rust/datafusion-wasm/Cargo.lock`
- `rust/datafusion-extensions/src/lib.rs`
- `rust/datafusion-extensions/tests/hll_tests.rs` (new)
- `rust/datafusion-extensions/src/hll/{mod,make_hll_udaf,merge_hll_udaf,cardinality}.rs` (new)
- `rust/datafusion-extensions/README.md`
- `mkdocs/docs/query-guide/functions-reference.md`
- `doc/how_to_query/README.md`
- `mkdocs/docs/query-guide/quick-start.md`
- `claude-plugin/skills/micromegas-query/SKILL.md`
- `CHANGELOG.md`

## Decisions

- Empty input (no non-null values) yields NULL rather than an empty serialized sketch, consistent
  with SQL aggregates such as sum/max; unlike `make_histogram`, which returns a zero-count
  histogram. `hll_cardinality(NULL)` is NULL, and `COALESCE(..., 0)` covers display.
- Keyed hashing deferred: fixed seed, no stable key in view definitions; document the trade-off
  and advise materializing sketches only for large groups.
- `lg_k` is fixed at 14 (~0.8% standard error) and not a `make_hll` argument; merge handles mixed
  precisions.
- AFK: empty strings/bytes are skipped (matches other DataSketches implementations).
- AFK: `UInt64` values above `i64::MAX` are hashed via wrapping cast to `i64`.
- `datasketches` is pinned at `0.5` (latest stable; MSRV 1.86 is met by the workspace
  toolchain). The 0.6 release candidate is not used.

## Documentation

- `functions-reference.md`: new "HyperLogLog Functions" section with signatures, return types, the
  accuracy (~0.8%), the DataSketches format/Spark compatibility note, the privacy trade-off, and a
  daily-uniques materialized view example (extract query with `make_hll`, merge query with
  `merge_hll`, and a MAU query with `hll_cardinality(merge_hll(...))`).
- Intersections are not supported (inclusion–exclusion is inaccurate); note this and compute
  intersections from raw data before it expires, or materialize a per-cohort sketch.

## Testing Strategy

No-DB tests in `rust/datafusion-extensions/tests/hll_tests.rs`:
- Accuracy: 100k distinct strings within ~3% (about 3.5 sigma of the 0.8% standard error); small
  exact-ish counts (list mode).
- Merge equals sketch of the union: merged sketches over two overlapping ranges give the same
  estimate as the sketch built over the union.
- Duplicates do not inflate; nulls ignored; all-null/empty group → NULL.
- Types: Utf8, Int64, Float64 accepted; Dictionary-encoded and Utf8View inputs give the same
  estimate as plain Utf8.
- Mixed `lg_k` merge: a sketch built at lg_k=12 via the crate merges with an lg_k=14 one; the
  lg_k=12 sketch must hold enough values to be in array mode so downsampling is exercised.
- Invalid bytes produce an error from `merge_hll` and `hll_cardinality`.
- Two-phase aggregation: run through a `SessionContext` with multiple partitions so `state` /
  `merge_batch` are exercised (in-memory tables, no external DB), including the
  materialized-view query shape: grouped `make_hll`, then `hll_cardinality(merge_hll(...))` over
  the grouped result.
- Binary and Utf8 hash agreement: `make_hll` over the same bytes as `Binary` and as `Utf8` yields
  identical sketches.
- wasm: covered by the CI wasm job (`./build/rust_ci.py wasm`).
