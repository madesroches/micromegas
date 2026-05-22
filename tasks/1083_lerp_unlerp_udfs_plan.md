# lerp / unlerp Scalar UDFs Plan

Issue: [#1083](https://github.com/madesroches/micromegas/issues/1083)

## Overview

Add two scalar UDFs for one-dimensional remapping:

- `lerp(a, b, t) -> Float64` — linear interpolation: `a + (b - a) * t`.
- `unlerp(a, b, x) -> Float64` — the dual: `(x - a) / (b - a)`.

They are the canonical pair for normalize-then-remap pipelines. Composed, `lerp(c, d, unlerp(a, b, x))` maps `[a,b] → [c,d]`. Each is the inverse of the other (`unlerp(a, b, lerp(a, b, t)) == t`). They make heatmap/colormap SQL self-documenting — the density example in the issue collapses an inline `COALESCE(... / NULLIF(... MAX(...) OVER ()), 1.0)` CTE into a single `unlerp` call, and the alpha ramp inside `color_scale(...)` becomes `lerp(0.5, $max_alpha, t)` instead of `0.5 + ($max_alpha - 0.5) * t`.

Sibling to the existing scalar UDFs `bin_center` (binning) and `lerp_color` (color) — same crate, same template.

## Current State

- **No scalar lerp/unlerp anywhere.** `lerp_color(c1, c2, t)` in `rust/datafusion-extensions/src/color/lerp_color.rs` exists for packed RGBA `u32` colors only — it interpolates channel-by-channel inside the pack, so it can't be reused for scalars. DataFusion's built-in scalar set (`abs`/`floor`/`ceil`/`round`/`power`/`sqrt`/`least`/`greatest`/`nanvl`/etc.) has no `lerp`/`unlerp`/`map_range` equivalent. Callers currently write the arithmetic inline.
- **UDF home.** Generic, WASM-compatible scalar UDFs live in `rust/datafusion-extensions/src/` and are registered in `register_extension_udfs()` (`rust/datafusion-extensions/src/lib.rs:47-84`). Every UDF topic is its own submodule folder (`binning/`, `color/`, `histogram/`, `jsonb/`, `properties/`). `lerp`/`unlerp` are scalar math, not color-specific, so a new `math/` folder is the right home — placing them in `color/` would mis-categorize `unlerp` (the issue's example uses it for density normalization, no colors involved).
- **Implementation pattern.** The closest precedent is the brand-new `rust/datafusion-extensions/src/binning/bin_center.rs` (97 lines): a struct deriving `#[derive(Debug, PartialEq, Eq, Hash)]` so the default `equals`/`hash_value` on `ScalarUDFImpl` work, an explicit `Signature::exact`, and an `invoke_with_args` that calls `ColumnarValue::values_to_arrays` (handles scalar→array expansion uniformly), downcasts each arg to `Float64Array`, and walks rows pushing into a `Float64Builder`. `lerp` and `unlerp` follow this template exactly with three `Float64` inputs instead of two.
- **Module-doc convention.** `binning/mod.rs` and `color/mod.rs` both lead with a doc-comment recording the API conventions (centered-on-zero, half-open intervals, no validation of pathological inputs / packing order, component range, straight alpha, sRGB). `math/mod.rs` will do the same: no clamping, IEEE NaN/Inf propagation, nulls propagate. Future math UDFs (`smoothstep`, `clamp_range`, `saturate`, `map_range`) reuse the same source of truth.
- **Test convention.** Per `CLAUDE.md`, tests live in the crate's `tests/` folder. The closest reference is `rust/datafusion-extensions/tests/bin_center_tests.rs`: build a `SessionContext`, call `register_extension_udfs(&ctx)`, run SQL, downcast the result column, assert. Uses an `eval_f64` helper that returns `Vec<Option<f64>>` — reused unchanged here.
- **Documentation.** The SQL functions reference is `mkdocs/docs/query-guide/functions-reference.md`. The latest topical group is `#### Binning Functions` at line 1236; `#### Math Functions` slots in after it, before `## Standard SQL Functions` at line 1268.

## Design

### API conventions (locked in by this plan)

Recorded in `math/mod.rs` so future math UDFs share one source of truth. Once external SQL exists in the wild these are effectively frozen — behaviour-changing variants must adopt a distinct name.

- **No clamping.** `lerp(a, b, t)` with `t` outside `[0,1]` extrapolates; `unlerp(a, b, x)` with `x` outside `[a,b]` returns a value outside `[0,1]`. `lerp_color` does clamp `t` (and clamps each channel via `round_to_byte`), but only because its `f64 → u8` pack would otherwise blow past the channel byte range — a quantization concern that the scalar form doesn't share. Callers who want clamping wrap with `LEAST(GREATEST(t, 0.0), 1.0)`.
- **IEEE-754 propagation, not errors.** `unlerp(a, a, x)` divides by zero and returns `NaN` (when `x == a`) or `±Inf` (when `x != a`). NaN/±Inf inputs propagate. No per-row erroring. Matches `bin_center`'s "pathological inputs are not validated" stance. Callers who want a fallback wrap with `nanvl(unlerp(...), 0.0)` (a DataFusion built-in).
- **Nulls propagate.** If any of the three inputs is `NULL`, the row's result is `NULL`. Matches every other scalar UDF in this crate.
- **Float64 only.** `Signature::exact(vec![Float64; 3], Volatility::Immutable)`. DataFusion's implicit numeric coercion lets callers write `lerp(0, 1, 0.5)` (int literals → `Float64`) without explicit casts — verified by the equivalent test in `bin_center_tests.rs::bin_center_accepts_int_literals_via_coercion`.

### Future-extension naming reservations

Not built now, recorded so the API stays coherent.

- **`smoothstep(edge0, edge1, x) -> Float64`** — HLSL/GLSL-style smooth Hermite interpolation. Composes with `lerp` the same way `unlerp` does (`smoothstep` is roughly `unlerp` followed by a cubic). Natural neighbor.
- **`map_range(x, a, b, c, d) -> Float64`** — Blender/Unreal-style one-shot remap. Considered for this issue and rejected (see Trade-offs) — leave the name reserved for future demand.
- **`saturate(x) -> Float64`** — HLSL-style `clamp(x, 0, 1)`. Natural sibling once anyone wants to clamp `t` post-`unlerp` without typing `LEAST(GREATEST(...))`.
- **`clamp(x, lo, hi)`** — already covered by DataFusion's `least`/`greatest`; do not re-add.

### Module layout

```
rust/datafusion-extensions/src/
  math/
    mod.rs       # module doc-comment with conventions; submodule decls
    lerp.rs      # lerp(a, b, t) -> Float64
    unlerp.rs    # unlerp(a, b, x) -> Float64
```

Matches the existing per-topic folder convention (`binning/`, `color/`, `histogram/`, `jsonb/`, `properties/`). Two siblings under `math/` from day one — `lerp` and `unlerp` are inseparable conceptually (the issue's whole point is the composition), and the folder anchors the future-extension naming reservations above.

### `lerp(a, b, t) -> Float64`

- **Signature:** `Signature::exact(vec![Float64; 3], Volatility::Immutable)`.
- **Return type:** `Float64`.
- **Behavior:**
  - `ColumnarValue::values_to_arrays` to expand any scalar literals to length-matched arrays (consistent with `bin_center`).
  - Downcast all three to `Float64Array`; error with `internal_err!` if the arity or types don't match the signature (defence in depth — DataFusion's planner should have rejected mismatches already).
  - For each row: if any input is null → append null. Otherwise compute `a + (b - a) * t` and append.
  - No validation. NaN/Inf propagate via float math.
- **Math note.** Written as `a + (b - a) * t`, not `a * (1 - t) + b * t`. The former is one fewer multiplication and is the form named in the issue and in `lerp_color.rs` (`a as f64 + (b as f64 - a as f64) * t`). It is *not* monotonic at the endpoints under floating-point — `lerp(a, b, 1.0)` does not always exactly equal `b` because `(b - a)` then `+ a` accumulates rounding. The alternative `(1 - t) * a + t * b` is monotonic but loses precision for small `(b - a)`. The issue specifies this formula, and the consumers (color ramps, alpha blends) tolerate sub-ULP endpoint drift; do not switch forms.

### `unlerp(a, b, x) -> Float64`

- **Signature:** `Signature::exact(vec![Float64; 3], Volatility::Immutable)`.
- **Return type:** `Float64`.
- **Behavior:**
  - Same arg expansion / downcasting / null propagation as `lerp`.
  - For each row: compute `(x - a) / (b - a)`. When `a == b`:
    - If `x == a`: result is `NaN` (0 / 0).
    - If `x != a`: result is `±Inf` based on sign.
    - Both are IEEE-754 natural; no special-casing in the impl.

### Registration

Append to `register_extension_udfs` in `rust/datafusion-extensions/src/lib.rs`:

```rust
ctx.register_udf(make_lerp_udf());
ctx.register_udf(make_unlerp_udf());
```

With a matching `use math::{lerp::make_lerp_udf, unlerp::make_unlerp_udf};` at the top, and `pub mod math;` in the module declarations block.

No changes needed in `rust/analytics/src/lakehouse/query.rs` — analytics already pulls in everything via `register_extension_udfs`.

## Implementation Steps

1. Add `pub mod math;` to `rust/datafusion-extensions/src/lib.rs` (alphabetically ordered with the other `pub mod` lines — between `jsonb` and `properties`).
2. Create `rust/datafusion-extensions/src/math/mod.rs` with the module doc-comment stating the conventions (no clamping, IEEE propagation, null propagation, Float64-only) plus `pub mod lerp; pub mod unlerp;`.
3. Implement `rust/datafusion-extensions/src/math/lerp.rs` — `LerpUdf` struct deriving `#[derive(Debug, PartialEq, Eq, Hash)]`, `ScalarUDFImpl` impl, `make_lerp_udf()` constructor. Model the file structure on `binning/bin_center.rs`.
4. Implement `rust/datafusion-extensions/src/math/unlerp.rs` — `UnlerpUdf` struct, `ScalarUDFImpl` impl, `make_unlerp_udf()` constructor. Same shape as `lerp.rs` with the divide instead of the multiply-add.
5. Register both UDFs in `register_extension_udfs()` (`lib.rs`). Place them after the color UDFs and before the binning UDF — color and math are upstream of binning conceptually, but ordering only affects readability since they all live in one registration block.
6. Add `rust/datafusion-extensions/tests/lerp_unlerp_tests.rs` (see Testing Strategy). Single file covers both UDFs since most of the value is in cross-checking the inverse property.
7. Add a `#### Math Functions` subsection to `mkdocs/docs/query-guide/functions-reference.md`, placed after `#### Binning Functions` (around line 1267, before `## Standard SQL Functions`). Match the existing entry style (Syntax / Parameters / Returns / Examples).
8. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and `cargo test -p micromegas-datafusion-extensions`.

No `Cargo.toml` description update needed — the existing string is "WASM-compatible DataFusion UDF extensions for micromegas" (already topic-agnostic; the earlier per-topic enumeration was dropped at some point).

## Files to Modify

- `rust/datafusion-extensions/src/lib.rs` — declare module, register both UDFs.
- `rust/datafusion-extensions/src/math/mod.rs` (new).
- `rust/datafusion-extensions/src/math/lerp.rs` (new).
- `rust/datafusion-extensions/src/math/unlerp.rs` (new).
- `rust/datafusion-extensions/tests/lerp_unlerp_tests.rs` (new).
- `mkdocs/docs/query-guide/functions-reference.md` — add Math Functions subsection.

## Trade-offs

- **`unlerp` vs. `inverse_lerp` vs. `map_range`.** `unlerp`. The issue settles this — `unlerp` matches shader-land vocabulary (where the name is dominant) and pairs symmetrically with `lerp` so the canonical remap `lerp(c, d, unlerp(a, b, x))` reads at a glance. `inverse_lerp` (Unity/HLSL) is more verbose. `map_range(x, a, b, c, d)` (Blender/Unreal) folds both into a single 5-arg call but loses the composability: the motivating density query uses `unlerp` for normalization in one CTE *separately* from the alpha ramp's `lerp` in another, and a `map_range` would force callers to factor the two stages together even when they live in different places. The name is still reserved for future use if demand appears.
- **`math/` folder vs. extending `color/`.** Folder. `unlerp` is not color-specific (the density example normalizes raw counts), and putting it in `color/` would mis-categorize half the API. Same call as the `bin_center` plan made for `binning/`.
- **Two siblings from day one vs. one file with both impls.** Two siblings. Matches the per-function-file precedent in `binning/`, `color/`, and `jsonb/`. Each impl is ~50 lines; collapsing them saves nothing and makes the future `smoothstep`/`saturate` additions less symmetric.
- **No clamping by default.** Matches `lerp_color`'s scalar behaviour (its only `t` clamp is required for the `f64 → u8` pack — a quantization concern, not a scalar one) and the issue's explicit request. SQL callers who want clamping have `LEAST/GREATEST`; baking it in would force everyone who wants extrapolation (occasionally legitimate — e.g. extending a calibration curve past its endpoints) to work around the UDF.
- **No per-row error on degenerate `unlerp(a, a, x)`.** Consistent with `bin_center`'s "pathological inputs are not validated" stance and with `nanvl`'s existence (DataFusion already gives callers a one-shot NaN fallback). Erroring per-row would also be wrong for the canonical use case `unlerp(0, MAX(cnt) OVER (), cnt)` when a window happens to contain zero rows or a single value — degeneracy is a normal data-shape condition, not an exceptional one.
- **`a + (b - a) * t` form vs. `(1 - t) * a + t * b`.** The former. Matches the issue's spec and `lerp_color`'s formula, and is one fewer multiplication. The endpoint-monotonicity penalty (sub-ULP drift at `t = 1.0`) is irrelevant for color and alpha consumers and is the trade-off `lerp_color` already locked in.
- **No vectorized N-D variant.** Same rationale as `bin_center`'s 1D-only choice. `lerp(a, b, t)` per axis composes trivially; a struct-returning UDF would force `LATERAL` syntax.

## Documentation

Add a new `#### Math Functions` group to `mkdocs/docs/query-guide/functions-reference.md`, placed after `#### Binning Functions` (line 1267, before `## Standard SQL Functions`). Two subsections — `lerp` and `unlerp` — matching the existing entry style:

````markdown
#### Math Functions

Scalar math helpers. `lerp` and `unlerp` are the canonical pair for normalize-then-remap pipelines: `lerp(c, d, unlerp(a, b, x))` maps the input range `[a,b]` to the output range `[c,d]`. Neither clamps; callers who want clamping wrap the result (e.g. `LEAST(GREATEST(t, 0.0), 1.0)`) or use the existing `nanvl(...)` to provide a fallback for degenerate `unlerp(a, a, x)` cases.

##### `lerp(a, b, t)`

Linear interpolation between `a` and `b`. Computes `a + (b - a) * t`. No clamping — `t` outside `[0, 1]` extrapolates past the endpoints.

**Syntax:**
```sql
lerp(a, b, t)
```

**Parameters:**

- `a` (`Float64`): Start of the output range.

- `b` (`Float64`): End of the output range.

- `t` (`Float64`): Interpolation parameter. `0.0` returns `a`, `1.0` returns `b`; values outside `[0,1]` extrapolate.

**Returns:** `Float64` — the interpolated value. `NULL` if any input is `NULL`; `NaN`/`±∞` propagate. Integer literals are accepted via DataFusion's implicit numeric coercion to `Float64`.

**Examples:**
```sql
-- Alpha ramp from 0.5 to 1.0 as t goes 0 → 1. Swap the second
-- argument for whatever maximum alpha the caller wants.
SELECT color_scale('inferno', t, lerp(0.5, 1.0, t)) AS color
FROM scaled;
```

##### `unlerp(a, b, x)`

Inverse linear interpolation. Computes `(x - a) / (b - a)` — i.e. the `t` such that `lerp(a, b, t) == x`. No clamping; `x` outside `[a, b]` returns a value outside `[0, 1]`.

`unlerp(a, a, x)` divides by zero and returns IEEE `NaN` (when `x == a`) or `±Inf` (when `x != a`). Wrap with `nanvl(unlerp(...), 0.0)` if a fallback is required.

**Syntax:**
```sql
unlerp(a, b, x)
```

**Parameters:**

- `a` (`Float64`): Start of the input range.

- `b` (`Float64`): End of the input range.

- `x` (`Float64`): Value to normalize.

**Returns:** `Float64` — the normalized position. `NULL` if any input is `NULL`; `NaN`/`±∞` propagate. Integer literals are accepted via DataFusion's implicit numeric coercion to `Float64`.

**Examples:**
```sql
-- Density normalization for a heatmap: t goes 0 → 1 across the visible range.
WITH scaled AS (
  SELECT cnt, unlerp(0.0, MAX(cnt) OVER (), CAST(cnt AS DOUBLE)) AS t
  FROM cells
)
SELECT cnt, t, color_scale('inferno', t, lerp(0.5, 1.0, t)) AS color
FROM scaled;
```
````

No update needed for `CLAUDE.md` or `AI_GUIDELINES.md`.

## Testing Strategy

`rust/datafusion-extensions/tests/lerp_unlerp_tests.rs`, following the `bin_center_tests.rs` pattern (build a `SessionContext`, register extension UDFs, run SQL, assert on the resulting `Float64Array`). Reuse the `eval_f64` helper verbatim.

Coverage:

**`lerp`:**
- **Endpoints.** `lerp(0.0, 10.0, 0.0) = 0.0`, `lerp(0.0, 10.0, 1.0) = 10.0`.
- **Midpoint.** `lerp(0.0, 10.0, 0.5) = 5.0`.
- **Extrapolation (no clamping).** `lerp(0.0, 10.0, 2.0) = 20.0`, `lerp(0.0, 10.0, -0.5) = -5.0`.
- **Reversed endpoints.** `lerp(10.0, 0.0, 0.25) = 7.5` (works in either direction).
- **Null propagation.** `lerp(NULL, 1.0, 0.5)`, `lerp(0.0, NULL, 0.5)`, `lerp(0.0, 1.0, NULL)` all return `NULL`.
- **Integer-literal coercion.** `lerp(0, 10, 0.5) = 5.0`.

**`unlerp`:**
- **Endpoints.** `unlerp(0.0, 10.0, 0.0) = 0.0`, `unlerp(0.0, 10.0, 10.0) = 1.0`.
- **Midpoint.** `unlerp(0.0, 10.0, 5.0) = 0.5`.
- **Outside the range.** `unlerp(0.0, 10.0, 15.0) = 1.5`, `unlerp(0.0, 10.0, -2.0) = -0.2`.
- **Degenerate `a == b`.** `unlerp(5.0, 5.0, 5.0)` returns `NaN`; `unlerp(5.0, 5.0, 7.0)` returns `+Inf`; `unlerp(5.0, 5.0, 3.0)` returns `-Inf`. Use `is_nan()` / `is_infinite()` on the returned `f64` rather than `assert_eq!` (NaN is not reflexively equal).
- **`nanvl` fallback works.** `nanvl(unlerp(5.0, 5.0, 5.0), 0.0) = 0.0` — confirms the documented fallback recipe survives the planner.
- **Null propagation.** Each of the three inputs as `NULL` yields `NULL`.
- **Integer-literal coercion.** `unlerp(0, 10, 5) = 0.5`.

**Composition / inverse property:**
- **`unlerp` is the inverse of `lerp` on `[0,1]`.** `unlerp(2.0, 8.0, lerp(2.0, 8.0, 0.3))` returns `0.3` (within `1e-12` to absorb FP drift).
- **`lerp` is the inverse of `unlerp`.** `lerp(2.0, 8.0, unlerp(2.0, 8.0, 4.5))` returns `4.5` (within `1e-12`).
- **Canonical remap.** `lerp(0.0, 1.0, unlerp(10.0, 20.0, 15.0)) = 0.5` — maps `[10, 20]` to `[0, 1]`.

**Column path:**
- **Scalar literals with column inputs.** A query like `SELECT lerp(0.0, 100.0, t) FROM (VALUES (0.0), (0.5), (1.0)) v(t)` returns `[0.0, 50.0, 100.0]`, exercising the scalar→array expansion path.

## Open Questions

None.
