# bin_center UDF Plan

Issue: [#1068](https://github.com/madesroches/micromegas/issues/1068)

## Overview

Add a scalar UDF `bin_center(coord, cell_size) -> Float64` that snaps a
coordinate to the center of its enclosing 1D bin. Spatial-binning queries
(heatmap/density layers over the map cell) call it twice — once per axis —
and `GROUP BY` the two results directly, producing `(x, y, cnt)` tuples that
the map renderer consumes without any awareness of grids.

Replaces the hand-rolled `FLOOR((x + cs/2) / cs) * cs` pattern from the
issue with a self-documenting expression that gets the centered-on-zero
offset right by construction.

## Current State

- **Spatial binning today.** No built-in UDF exists. Notebooks that need a
  heatmap reinvent the `FLOOR((x + cs/2) / cs)` math inline; the issue
  example is the canonical form. No existing SQL in the repo uses it (the
  current default map query in
  `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` is
  `SELECT NOW() as time, 0.0 as x, 0.0 as y, 0.0 as z`), so we are setting
  the convention rather than retrofitting one.
- **Renderer is grid-agnostic.** `analytics-web-app/src/components/map/overlay.ts:209-443`
  reads columns by name from `OverlayMapping` (defaults `x`, `y`, `z`,
  plus optional `size`/`color`) and renders one instanced marker per row.
  Nothing in `MapViewer.tsx` or `overlay.ts` understands `cell_size`, `ix/iy`,
  or any grid concept — it just consumes continuous `(x, y, z)` floats. A
  query that does `GROUP BY bin_center(x, $cs), bin_center(y, $cs)` produces
  exactly the shape the renderer already expects, with no renderer changes.
- **UDF home.** Generic, WASM-compatible scalar UDFs live in
  `rust/datafusion-extensions/src/` and are registered in
  `register_extension_udfs()` (`rust/datafusion-extensions/src/lib.rs:42-76`).
  Every existing UDF topic (`color`, `histogram`, `jsonb`, `properties`) is
  its own submodule folder, even ones that started with a single function
  (`color/` was created with two; the rationale in
  `tasks/completed/1062_color_udfs_plan.md` was explicitly to anticipate
  growth). A `binning/` folder follows that precedent — `bin_center` is the
  first inhabitant; future siblings (`bin_index`, shifted-origin variants)
  fit naturally.
- **Implementation pattern.** The closest precedent is
  `rust/datafusion-extensions/src/color/rgba.rs`: a struct deriving
  `#[derive(Debug, PartialEq, Eq, Hash)]` (so the default `equals`/`hash_value`
  on `ScalarUDFImpl` work), an explicit `Signature::exact`, and an
  `invoke_with_args` that downcasts inputs to `Float64Array`, walks rows,
  and pushes into a builder. `bin_center` follows that template exactly,
  with the wrinkle that `cell_size` is *typically* a scalar literal — but
  the implementation must still handle the columnar case correctly (a
  per-row `cell_size` column is unusual but legal).
- **Test convention.** Per `CLAUDE.md`, tests live in the crate's `tests/`
  folder. The closest reference is
  `rust/datafusion-extensions/tests/color_tests.rs`: build a `SessionContext`,
  call `register_extension_udfs(&ctx)`, run SQL, downcast the result column,
  assert.
- **Documentation.** The SQL functions reference is
  `mkdocs/docs/query-guide/functions-reference.md`. The latest topical group
  is "Color Functions" at line 1088; "Binning Functions" slots in after it.

## Design

### API convention (locked in by this plan)

Recorded in the `binning/mod.rs` module doc-comment so future binning UDFs
share one source of truth. Once external SQL exists in the wild these are
effectively frozen — behaviour-changing variants must adopt a distinct
name.

- **Centered on zero.** `bin_center(0, cs) = 0`. The bin containing
  `coord` spans the half-open interval `[c - cs/2, c + cs/2)` where
  `c = bin_center(coord, cs)`. This is the convention the issue calls out
  as non-obvious from the raw formula.
- **Half-open intervals.** A point that lands exactly on a bin edge
  (`coord = c + cs/2`) belongs to the *next* bin. This matches the standard
  `FLOOR` convention and is what histogram code in the same crate already
  assumes (`histogram/accumulator.rs` uses `floor((v - start) / bin_width)`
  bucketing — inclusive-low/exclusive-high between adjacent bins).
- **Pathological inputs.** `cell_size <= 0` and `NaN`/`±∞` inputs are
  *not* validated. The float math propagates `NaN`/`Inf` naturally; a
  zero or negative `cell_size` produces undefined results. Documented as a
  precondition. Erroring per row would add overhead and complicate the
  scalar-vs-array path; the closest precedent (`rgba`) likewise clamps/
  propagates rather than erroring.

### Future-extension naming reservations

Not built now, recorded so the API stays coherent.

- **Index accessor:** `bin_index(coord, cs) -> Int64` — the raw `floor(...)`
  result, for sparse-key use cases (e.g. joining against a precomputed
  grid). Out of scope for #1068 because grouping by `Float64` bin centers
  in DataFusion is essentially the same cost as grouping by `Int64` (same
  8-byte hash work), and the centers are bit-deterministic given identical
  inputs, so no group-splitting risk.
- **Vectorized 2D / N-D:** Deliberately *not* offered. The 1D form composes
  trivially (`bin_center(x, cs), bin_center(y, cs)`), and a struct-returning
  UDF would force callers into `LATERAL` syntax that no other UDF in this
  crate uses.

### Module layout

```
rust/datafusion-extensions/src/
  binning/
    mod.rs         # module doc-comment with conventions; submodule decls
    bin_center.rs  # bin_center(coord, cell_size) -> Float64
```

Matches the existing per-topic folder convention (`color/`, `histogram/`,
`jsonb/`, `properties/`).

### `bin_center(coord, cell_size) -> Float64`

- **Signature:**
  `Signature::exact(vec![Float64, Float64], Volatility::Immutable)`.
  DataFusion's implicit numeric coercion handles `Int64`/`Float32` callers
  without explicit casts (consistent with how `rgba` accepts `rgba(1, 0, 0, 1)`).
- **Return type:** `Float64`.
- **Behavior:**
  - Downcast both args to `Float64Array` after `ColumnarValue::values_to_arrays`
    (consistent with `rgba`). This handles scalar `cell_size` literals
    correctly — `values_to_arrays` rewrites scalars into length-matched
    arrays — so the inner loop sees a uniform shape.
  - For each row: if either input is null → result is null. Otherwise
    compute `((coord + cell_size * 0.5) / cell_size).floor() * cell_size`
    and append to a `Float64Builder`.
  - No validation of `cell_size`. NaN/Inf propagate; non-positive values
    produce undefined results (documented).
- **Math note.** The formula is written `coord + cell_size * 0.5` rather
  than `coord + cell_size / 2` for symmetry with how the issue prose
  describes "half a cell". Both produce identical IEEE-754 results, but
  `* 0.5` makes the half-step explicit in the source.

### Registration

Append to `register_extension_udfs` in
`rust/datafusion-extensions/src/lib.rs`:

```rust
ctx.register_udf(make_bin_center_udf());
```

With a matching
`use binning::bin_center::make_bin_center_udf;` at the top, and
`pub mod binning;` in the module declarations block.

No changes needed in `rust/analytics/src/lakehouse/query.rs` — analytics
already pulls in everything via `register_extension_udfs`.

## Implementation Steps

1. Add `pub mod binning;` to `rust/datafusion-extensions/src/lib.rs`.
2. Create `rust/datafusion-extensions/src/binning/mod.rs` with the module
   doc-comment stating the conventions (centered on zero, half-open
   intervals, no validation of pathological inputs) plus
   `pub mod bin_center;`.
3. Implement `rust/datafusion-extensions/src/binning/bin_center.rs` —
   `BinCenterUdf` struct deriving `#[derive(Debug, PartialEq, Eq, Hash)]`,
   `ScalarUDFImpl` impl, `make_bin_center_udf()` constructor. Model the
   file structure on `color/rgba.rs`.
4. Register the UDF in `register_extension_udfs()` (`lib.rs`).
5. Add `rust/datafusion-extensions/tests/bin_center_tests.rs` (see Testing
   Strategy).
6. Add a `#### Binning Functions` subsection to
   `mkdocs/docs/query-guide/functions-reference.md`, placed after
   `#### Color Functions` (around line 1162, before
   `## Standard SQL Functions`). Match the existing entry style (Syntax /
   Parameters / Returns / Examples).
7. Update the `description` field in
   `rust/datafusion-extensions/Cargo.toml` to include "binning" alongside
   the existing tags.
8. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and
   `cargo test -p micromegas-datafusion-extensions`.

## Files to Modify

- `rust/datafusion-extensions/Cargo.toml` — add "binning" to the crate
  description.
- `rust/datafusion-extensions/src/lib.rs` — declare module, register UDF.
- `rust/datafusion-extensions/src/binning/mod.rs` (new).
- `rust/datafusion-extensions/src/binning/bin_center.rs` (new).
- `rust/datafusion-extensions/tests/bin_center_tests.rs` (new).
- `mkdocs/docs/query-guide/functions-reference.md` — add Binning Functions
  subsection.

## Trade-offs

- **1D scalar UDF vs. 2D struct-returning UDF.** The issue's original
  proposal sketched `bin2d(x, y, cs) -> STRUCT { ix, iy, cx, cy }`. Rejected:
  no struct-returning UDF exists in this crate yet; `LATERAL` syntax is
  heavier than callers need; coupling x and y forecloses the 1D use case
  (time bucketing, value histograms) for no real gain. The 1D form
  composes trivially — `bin_center(x, cs), bin_center(y, cs)` — and reads
  the same way the renderer's column mapping does.
- **Return cell center vs. integer index.** Center. The motivating
  consumer (map cell) wants continuous `(x, y)` coordinates, and grouping
  by `Float64` centers in DataFusion costs the same as grouping by
  `Int64` indices (same 8-byte hash work) with the same row count. Returning
  the index would force callers into a `GROUP BY ix, iy; SELECT first_value(cx)`
  pattern that adds a per-row aggregator without saving anything. A separate
  `bin_index` UDF can come later for sparse-key use cases.
- **`binning/` folder vs. flat `bin_center.rs`.** Folder, matching the
  existing per-topic convention even though only one function lives there
  initially. Same call as the color plan.
- **No `cell_size <= 0` validation.** Consistent with `rgba`'s "clamp/
  propagate, don't error" stance. A per-row error path complicates the
  scalar/array dispatch and makes the function inconsistent with its
  neighbors. The docs call out the precondition.
- **Name: `bin_center` vs. `cell_center` vs. `bin2d`.** `bin_center`.
  Matches the histogram/data-analysis vocabulary the rest of the crate
  already uses ("bin edges", "bin width", "bin center" — see the
  `histogram` module), reads naturally for 1D and 2D callers, and avoids
  the `cell` GIS connotation. `bin2d` is rejected because the function is
  1D; the 2D pattern is two calls, which is exactly the renderer-agnostic
  ergonomic we want.

## Documentation

Add a new `#### Binning Functions` group to
`mkdocs/docs/query-guide/functions-reference.md`, placed after
`#### Color Functions` (line 1162). One subsection for `bin_center`,
matching the existing template:

````markdown
#### Binning Functions

##### `bin_center(coord, cell_size)`

Snaps a coordinate to the center of its enclosing 1D bin. Bins are
centered on zero (`bin_center(0, cs) = 0`) with width `cell_size`; the bin
containing `coord` spans the half-open interval `[c - cs/2, c + cs/2)`
where `c` is the returned center. Call once per axis to build a 2D grid;
the result is a continuous coordinate pair that map cells (and other
position-aware consumers) render the same way they render raw points.

**Syntax:**
```sql
bin_center(coord, cell_size)
```

**Parameters:**

- `coord` (`Float64`): Coordinate to snap.
- `cell_size` (`Float64`): Bin width. Must be positive; behaviour is
  undefined for non-positive values.

**Returns:** `Float64` — the bin center. `NULL` if either input is `NULL`;
`NaN`/`±∞` inputs propagate.

**Examples:**

```sql
-- 2D density grid over map events. Renderer sees (x, y, cnt) the same
-- way it sees raw points — no awareness of "cells" required.
SELECT bin_center(x, 50.0) AS x,
       bin_center(y, 50.0) AS y,
       COUNT(*) AS cnt
FROM events
GROUP BY 1, 2;
```
````

No update needed for `CLAUDE.md` or `AI_GUIDELINES.md`.

## Testing Strategy

`rust/datafusion-extensions/tests/bin_center_tests.rs`, following the
`color_tests.rs` pattern (build a `SessionContext`, register extension
UDFs, run SQL, assert on the resulting `Float64Array`).

Coverage:

- **Origin.** `bin_center(0.0, 10.0) = 0.0`.
- **Inside a bin, no rounding.** `bin_center(3.0, 10.0) = 0.0`
  (because `3` lies in `[-5, 5)`).
- **Negative side.** `bin_center(-3.0, 10.0) = 0.0`,
  `bin_center(-5.0, 10.0) = 0.0` (the bin centered at `0` is `[-5, 5)`;
  the lower bound is inclusive, so `-5` falls into that bin),
  `bin_center(-5.0001, 10.0) = -10.0` (just below the lower edge falls
  into the next bin down, `[-15, -5)`).
- **Upper edge of a bin lands in the next bin.**
  `bin_center(5.0, 10.0) = 10.0` (half-open: `5` belongs to `[5, 15)`).
- **Two-axis composition matches the motivating use case.**
  ```sql
  SELECT bin_center(x, 10.0) AS bx, bin_center(y, 10.0) AS by
  FROM (VALUES (3.0, 7.0), (-2.0, 12.0), (4.99, 4.99)) t(x, y)
  ```
  asserts the resulting `(bx, by)` pairs are `(0, 10)`, `(0, 10)`, `(0, 0)`.
- **Null propagation.** `bin_center(NULL, 10.0)` and `bin_center(3.0, NULL)`
  both return `NULL`.
- **Integer-literal coercion.** `bin_center(3, 10) = 0.0` (callers should
  not need explicit casts; DataFusion's implicit `Int64 → Float64`
  coercion handles it).
- **Scalar `cell_size` literal vs. column.** A query mixing a constant
  literal `cell_size` with a column-valued `coord` returns one bin per row
  (sanity check that the scalar→array expansion path doesn't broadcast
  incorrectly).
- **Group-by smoke test.** A `GROUP BY bin_center(x, cs), bin_center(y, cs)`
  over an inline `VALUES` source produces the expected number of output
  rows. Demonstrates the renderer-facing shape and acts as a regression
  guard against any future signature change that would break grouping.

## Open Questions

None.
