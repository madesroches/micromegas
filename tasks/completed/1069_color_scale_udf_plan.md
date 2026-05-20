# Color Scale UDF Plan

Issue: [#1069](https://github.com/madesroches/micromegas/issues/1069)

## Overview

Add a `color_scale(name, t, alpha) -> UInt32` scalar UDF that produces a
packed RGBA color from a perceptually-uniform color scale (viridis, magma,
plasma, inferno, turbo). Today the natural default for a heat-style overlay
in a notebook map cell is `lerp_color(rgba(0,0,1,a), rgba(1,0,0,a), t)`,
which has a muddy purple mid-band, flat luminance, and bad accessibility
behavior. This UDF replaces that 3-line invocation with one function call
and gives users built-in perceptual scales with the same return type the
rest of the color UDFs use.

## Current State

- **Color UDF crate.** WASM-compatible scalar color UDFs live in
  `rust/datafusion-extensions/src/color/`. Two UDFs already ship:
  `rgba(r,g,b,a)` (`src/color/rgba.rs`) and `lerp_color(c1,c2,t)`
  (`src/color/lerp_color.rs`). Both implement `ScalarUDFImpl` and follow
  the `make_<name>_udf() -> ScalarUDF` constructor pattern.
- **Locked-in conventions.** `src/color/mod.rs:3-30` lays out the four
  API conventions every color UDF must honor (packing as `0xRRGGBBAA`,
  float channels in `[0,1]` with clamp at byte boundary, straight alpha,
  sRGB color space). The future-extension reservations section
  (lines 22-30) does not yet name colormap UDFs — this plan adds that
  reservation.
- **Shared helpers.** `pack_rgba`, `unpack_rgba`, `float_to_byte`, and
  `round_to_byte` are exported from `color/mod.rs`. The new UDF reuses
  `pack_rgba` and `float_to_byte` for assembling the final `u32`.
- **Registration.** Extension UDFs register in
  `rust/datafusion-extensions/src/lib.rs:45-81` via
  `register_extension_udfs(&ctx)`. The new UDF appends one line there.
- **Test pattern.** Tests live in `rust/datafusion-extensions/tests/`
  per `CLAUDE.md`. `color_tests.rs` already builds a `SessionContext`,
  registers extension UDFs, evals SQL through a `eval_u32(ctx, expr)`
  helper, and asserts on returned `Option<u32>` values. The new tests
  reuse that helper.
- **Documentation.** SQL functions live in
  `mkdocs/docs/query-guide/functions-reference.md`. The existing
  `#### Color Functions` group (line 1088) explains the packing
  convention and documents `rgba` and `lerp_color`. The new UDF adds a
  `##### color_scale(name, t, alpha)` subsection under that group.
- **Consumer.** The map cell decodes `0xRRGGBBAA` packed `u32` in
  `analytics-web-app/src/lib/screen-renderers/cells/MapViewer.tsx`
  (per the [1062 plan](completed/1062_color_udfs_plan.md)). The new UDF
  returns the same `UInt32` layout, so no consumer-side changes are
  needed.

## Design

### API shape (locked in by this plan)

Following the issue's preferred form:

```
color_scale(name: Utf8, t: Float64, alpha: Float64) -> UInt32
```

- `name` — colormap identifier; recognized values are listed below.
  Matched case-insensitively against the canonical lowercase name.
- `t` — position along the scale, clamped to `[0.0, 1.0]`.
- `alpha` — output alpha channel, clamped to `[0.0, 1.0]` and quantized
  to `0..=255` via `float_to_byte` (the same helper `rgba` uses, so
  alpha quantization stays consistent across color UDFs).
- Return — packed `0xRRGGBBAA` `UInt32`. NULL if any of the three
  inputs is NULL on that row. Unknown colormap names raise an error
  (see *Name validation* below).
- Alpha is independent of the colormap's RGB output. Colormap tables
  store RGB only; the user picks alpha freely. This matches `rgba`'s
  straight-alpha convention.

**Canonical colormap names (initial set):**

| Name      | Family    | Use case                          |
| --------- | --------- | --------------------------------- |
| `viridis` | sequential, blue→green→yellow | default heatmap, peak findability |
| `magma`   | sequential, black→red→yellow  | dark-backdrop overlays            |
| `plasma`  | sequential, purple→orange→yellow | high-contrast sequential       |
| `inferno` | sequential, black→red→yellow  | dark-backdrop, hotter mid-band    |
| `cividis` | sequential, blue→yellow       | maximum color-vision-deficiency safety |
| `turbo`   | "rainbow-style" but perceptual | when categorical-looking contrast is wanted |

The five from the issue (viridis, magma, plasma, inferno, turbo) plus
cividis, which is the canonical "color-vision-safe sequential" scale
and fits the same `color_scale` signature with no extra design work.
All are perceptually-uniform with monotonic luminance (turbo
near-monotonic). Names match matplotlib/d3/colorous.

### Signature & coercion

```rust
Signature::exact(
    vec![DataType::Utf8, DataType::Float64, DataType::Float64],
    Volatility::Immutable,
)
```

`Volatility::Immutable` enables DataFusion to constant-fold the call
when all three args are constants. DataFusion will coerce `LargeUtf8` /
`Utf8View` to `Utf8` and `Int64` / `Float32` to `Float64` under
`exact`, so `color_scale('viridis', 1, 1)` works without explicit
casts. The expected planning behaviour mirrors `rgba` (see
`tests/color_tests.rs:96-103`).

### Lookup strategy

Use the `colorous` crate (Apache-2.0, no runtime dependencies, pure
Rust → WASM-compatible) as the source of colormap data. It exposes a
single function we need:

```rust
colorous::VIRIDIS.eval_continuous(t: f64) -> colorous::Color { r: u8, g: u8, b: u8 }
```

`colorous` already supplies viridis, magma, plasma, inferno, and turbo
with values traceable to matplotlib (BIDS team) and Google. Using the
crate avoids hand-maintaining 5 × 256 × 3 = 3840 bytes of color data in
the repo and keeps the source authoritative.

Trade-off versus owning the tables is recorded in *Trade-offs* below.

### Module layout

Add one new file in the existing `color/` directory, mirroring the
sibling layout used by `rgba.rs` and `lerp_color.rs`:

```
rust/datafusion-extensions/src/color/
  mod.rs           # add color_scale module decl + colormap reservation in doc
  rgba.rs          # existing
  lerp_color.rs    # existing
  color_scale.rs   # NEW: ColorScaleUdf + make_color_scale_udf()
```

No new submodule directory — the UDF is small and self-contained, and
the existing color/ flat layout has been working well.

### Internal lookup

```rust
fn resolve_colormap(name: &str) -> Option<colorous::Gradient> {
    // Lowercase once; SQL string literals are case-sensitive but we
    // forgive ALL CAPS / Mixed Case to keep the API ergonomic.
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "viridis" => Some(colorous::VIRIDIS),
        "magma"   => Some(colorous::MAGMA),
        "plasma"  => Some(colorous::PLASMA),
        "inferno" => Some(colorous::INFERNO),
        "cividis" => Some(colorous::CIVIDIS),
        "turbo"   => Some(colorous::TURBO),
        _ => None,
    }
}
```

`colorous::Gradient` is `Copy` and the gradient constants are `pub const`
items (no `'static` reference available), so the helper returns by value
rather than by reference.

### Per-row evaluation

```rust
let c = gradient.eval_continuous(t.clamp(0.0, 1.0));
let a = float_to_byte(alpha);     // shared with rgba
pack_rgba(c.r, c.g, c.b, a)       // shared with rgba/lerp_color
```

### Name validation timing

Validate the colormap name in `invoke_with_args` once at the start of
the call, optimizing for the common "name is a literal" case:

1. Inspect `args.args[0]` directly *before* the
   `ColumnarValue::values_to_arrays` lowering. If it is
   `ColumnarValue::Scalar(ScalarValue::Utf8(Some(s)))`, resolve once;
   error out immediately on unknown name. With `Volatility::Immutable`,
   a fully-literal call (`color_scale('virids', 0.5, 1.0)`) gets
   constant-folded by DataFusion's `ConstEvaluator` and surfaces this
   error at plan time; the more common `color_scale('virids', column_t,
   1.0)` form is not foldable, so the error fires once on the first
   `RecordBatch` instead of per row.
2. Otherwise (column-driven name, rare in practice), fall through to
   the row loop and resolve per row. Unknown names on any row produce
   a query-time error mentioning the offending name and the recognized
   set.

The error message lists the recognized names so users can fix the
typo without consulting docs.

### Behaviour summary

| Input                                            | Output              |
| ------------------------------------------------ | ------------------- |
| `color_scale('viridis', 0.0, 1.0)`               | `0x440154ff`        |
| `color_scale('viridis', 1.0, 1.0)`               | `0xfde725ff`        |
| `color_scale('Viridis', 0.5, 1.0)`               | same as `'viridis'` |
| `color_scale('viridis', -0.5, 1.0)`              | clamp → `t=0.0`     |
| `color_scale('viridis', 1.5, 0.5)`               | clamp → `t=1.0`, a=128 |
| `color_scale(NULL, 0.5, 1.0)`                    | `NULL`              |
| `color_scale('viridis', NULL, 1.0)`              | `NULL`              |
| `color_scale('viridis', 0.5, NULL)`              | `NULL`              |
| `color_scale('not_a_map', 0.5, 1.0)`             | error before row loop (plan time when constant-folded) |

### Future-extension reservations

Document in `color/mod.rs` so future contributors don't pick conflicting
names:

- **Categorical scales** (e.g. `tab10`, `set1`) — separate UDF
  `color_category(name, i, alpha) -> UInt32` taking an integer index
  rather than a `t`. Different signature → different name.
- **Diverging scales** (e.g. `RdBu`, `coolwarm`) — fit under
  `color_scale` once they are added to the recognized set; document a
  midpoint = 0.5 convention.
- **Reversed scales** — add a `color_scale_reversed(name, t, alpha)`
  helper, or accept a `'viridis_r'` suffix in the name; pick when the
  first user asks. Reserved, not built.
- **Direct exposure as individual UDFs** (e.g. `viridis(t, alpha)`) —
  intentionally *not* reserved. The plan keeps a single
  `color_scale` dispatch UDF to avoid polluting the function namespace
  with per-colormap UDFs; if a future need arises, the individual names
  can be added as aliases.

## Implementation Steps

1. **Add `colorous` to workspace dependencies.**
   - `rust/Cargo.toml`: add `colorous = "1.0"` (latest stable, Apache-2.0)
     to `[workspace.dependencies]` in alphabetical order.
   - `rust/datafusion-extensions/Cargo.toml`: add
     `colorous.workspace = true` under `[dependencies]` in alphabetical
     order. Update the crate `description` to mention color scales
     (e.g. append "with built-in perceptually-uniform color scales").
2. **Add the UDF source file.** Create
   `rust/datafusion-extensions/src/color/color_scale.rs`:
   - `ColorScaleUdf` struct with `#[derive(Debug, PartialEq, Eq, Hash)]`
     and `Signature` field (same idiom as `RgbaUdf` and `LerpColorUdf`).
   - `ScalarUDFImpl` impl: `name()` returns `"color_scale"`,
     `return_type` returns `UInt32`, `invoke_with_args` does the
     scalar-name fast path then per-row evaluation.
   - Private `resolve_colormap(&str) -> Option<colorous::Gradient>`
     helper (returns by value — `Gradient` is `Copy` and the colorous
     gradient items are `pub const`, not `static`).
   - `make_color_scale_udf() -> ScalarUDF` constructor.
3. **Wire it up.**
   - `rust/datafusion-extensions/src/color/mod.rs`:
     - Add `pub mod color_scale;` next to the existing two.
     - Add a "Colormaps" line to the future-extension reservations
       section in the module doc-comment, noting that `color_scale`
       is the entry point and that categorical/reversed/diverging
       variants are reserved (per the design above).
   - `rust/datafusion-extensions/src/lib.rs`:
     - Add `color_scale::make_color_scale_udf` to the `use color::{...}`
       group.
     - Append `ctx.register_udf(make_color_scale_udf());` after the two
       existing color registrations (`lib.rs:77-78`).
4. **Add tests.** Append cases to
   `rust/datafusion-extensions/tests/color_tests.rs` (do not create a
   new file — the existing file already covers the `color` module).
   See *Testing Strategy*.
5. **Update SQL function docs.** Append a
   `##### color_scale(name, t, alpha)` subsection at the end of the
   existing `#### Color Functions` section in
   `mkdocs/docs/query-guide/functions-reference.md`, between
   `lerp_color` and `#### Binning Functions`. Match the existing
   Syntax / Parameters / Returns / Examples template.
6. **Run CI locally.** From `rust/`:
   - `cargo fmt`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test -p micromegas-datafusion-extensions`
   - `python3 ../build/rust_ci.py`

## Files to Modify

- `rust/Cargo.toml` — add `colorous` workspace dep.
- `rust/datafusion-extensions/Cargo.toml` — add `colorous.workspace =
  true`; update crate description.
- `rust/datafusion-extensions/src/color/mod.rs` — add module decl,
  extend future-extension reservation doc.
- `rust/datafusion-extensions/src/color/color_scale.rs` — NEW.
- `rust/datafusion-extensions/src/lib.rs` — import + register UDF.
- `rust/datafusion-extensions/tests/color_tests.rs` — append test
  cases for `color_scale`.
- `mkdocs/docs/query-guide/functions-reference.md` — add
  `color_scale` subsection.

## Trade-offs

- **Single dispatch UDF (`color_scale(name, ...)`) vs. per-colormap
  UDFs (`viridis(t, alpha)`, …).** Single UDF chosen. The issue offers
  both options; single dispatch keeps the function namespace tight
  ("one new SQL function for a whole family"), survives the addition
  of more scales without growing the registration list, and matches
  the d3 `interpolateXxx`/`scaleSequential` and matplotlib
  `get_cmap('name')` style users may already know. Cost is that a
  typo in the name surfaces at query time rather than function-name
  resolution time — mitigated by the fast-path validation that
  detects literal-name typos at plan time.
- **`colorous` dependency vs. owning the colormap tables.** Take
  the dep. It is Apache-2.0, has no runtime dependencies, is a thin
  data-only crate (~5 KB of color tables and a handful of methods),
  and is the canonical Rust port of the matplotlib/d3 colormap data.
  Owning ~4 KB of `[u8; 3]` table data ourselves would add a
  generator script, an audit responsibility for any future
  regeneration, and a place future drift can sneak in. The crate is
  small enough to vendor mentally — `eval_continuous(t)` and that's
  it.
- **Case-insensitive name match vs. strict lowercase.** Case-
  insensitive. The lookup is a one-shot `to_ascii_lowercase` per call
  and forgives the most common user mistake without a docs detour.
  Documentation still shows lowercase as canonical.
- **Error vs. NULL on unknown colormap name.** Error. The set is small
  and finite; an unrecognized name is almost certainly a typo, not a
  data condition. Returning NULL would hide the bug.
- **Linear interpolation between table entries (what `colorous` does)
  vs. nearest-neighbor lookup.** Interpolation — preserves smooth
  gradients. `colorous::eval_continuous` already does this; nothing
  to decide on our side.
- **NaN handling.** No special case. `f64::clamp` propagates NaN, so
  NaN `t` falls through to `colorous::eval_continuous` and NaN
  `alpha` falls through to `float_to_byte`; in both paths the
  saturating `f64 as u8` cast resolves NaN to 0. Matches the
  existing family's "no NaN special-case" stance (`lerp_color` does
  the same). Adding NaN logic was rejected as unnecessary surface.
- **No alpha default.** `rgba` already requires an explicit alpha;
  matching that here keeps the family consistent and dodges
  variadic-signature complexity. Users who want full opacity write
  `color_scale('viridis', t, 1.0)`.
- **`color_scale` vs. `colormap` as the function name.** The issue
  proposes `color_scale`; d3 says "scale," matplotlib says "cmap,"
  ggplot says "scale." Naming follows the issue. `colormap` stays
  available as an alias if a future need surfaces.

## Documentation

Add the following entry to
`mkdocs/docs/query-guide/functions-reference.md`, immediately after
the existing `##### lerp_color(c1, c2, t)` subsection and before
`#### Binning Functions`:

````markdown
##### `color_scale(name, t, alpha)`

Samples a built-in perceptually-uniform color scale, returning a
packed RGBA `UInt32` in `0xRRGGBBAA` byte order.

**Syntax:**
```sql
color_scale(name, t, alpha)
```

**Parameters:**

- `name` (`Utf8`): Color scale identifier. Recognized values
  (case-insensitive):
  - `viridis` — blue → green → yellow (default for general heatmaps;
    monotonic luminance, color-vision safe)
  - `magma`   — black → red → yellow (good over dark backdrops)
  - `plasma`  — purple → orange → yellow (high contrast)
  - `inferno` — black → red → yellow (dark backdrops, hotter mid-band)
  - `cividis` — blue → yellow (maximum color-vision-deficiency safety)
  - `turbo`   — rainbow-style, perceptually corrected

- `t` (`Float64`): Position along the scale, clamped to `[0.0, 1.0]`.

- `alpha` (`Float64`): Output alpha channel, `[0.0, 1.0]` (clamped),
  straight (not premultiplied) — independent of the scale's RGB.

**Returns:** `UInt32` — packed color. `NULL` if any input is `NULL`.
An unrecognized `name` raises an error.

**Examples:**
```sql
-- Density overlay with a perceptual scale; replaces the blue→red lerp
-- that has a muddy purple mid-band and poor accessibility.
SELECT x, y,
       color_scale('viridis', value / max_value, 0.7) AS color
FROM density_grid;

-- Dark-mode map cell: magma keeps the hottest cell bright yellow.
SELECT x, y,
       color_scale('magma', t, 1.0) AS color
FROM heatmap;

-- Pure turbo lookup (alpha = 1).
SELECT color_scale('turbo', 0.5, 1.0);  -- mid-band turbo color
```
````

No update required to `CLAUDE.md` or `AI_GUIDELINES.md`.

## Testing Strategy

Append cases to `rust/datafusion-extensions/tests/color_tests.rs`,
reusing the existing `make_ctx()` / `eval_u32()` helpers. Coverage:

- **Endpoint anchors per colormap.** For each of viridis / magma /
  plasma / inferno / cividis / turbo: assert `color_scale(name, 0.0, 1.0)` and
  `color_scale(name, 1.0, 1.0)` against the canonical hex values
  reported by `colorous`. (Compute them once during test authoring by
  calling `colorous::VIRIDIS.eval_continuous(0.0)` etc.; bake the
  expected `u32` literals into the test. This guards against an
  upstream crate update silently shifting the gradient — if `colorous`
  changes its data, our tests fail loudly and we either pin a version
  or refresh the constants.)
- **Alpha is honoured.** `color_scale('viridis', 0.0, 0.5)` →
  same RGB as `color_scale('viridis', 0.0, 1.0)` with the low byte
  equal to `128`.
- **Alpha clamping.** `alpha = -0.5` and `alpha = 1.5` produce `0` and
  `255` respectively in the low byte.
- **t clamping.** `t = -0.5` matches `t = 0.0`; `t = 1.5` matches
  `t = 1.0`. Mirrors the existing `lerp_color` clamp tests.
- **Case-insensitive name match.** `color_scale('Viridis', 0.5, 1.0)`
  and `color_scale('VIRIDIS', 0.5, 1.0)` both equal
  `color_scale('viridis', 0.5, 1.0)`.
- **Null inputs.** Each of name / t / alpha → null result, mirroring
  the existing `rgba_null_input_yields_null` and
  `lerp_color_null_inputs_yield_null` patterns.
- **Unknown name → error.** `color_scale('not_a_scale', 0.5, 1.0)`
  fails the SQL call. Assert `.is_err()` (matching the
  `lerp_color_rejects_bare_int_literals` test pattern at
  `color_tests.rs:217-226`). Verify the error message names the
  bad colormap and lists the recognized set — that's the load-bearing
  user-facing detail that makes the error helpful rather than
  cryptic.
- **Integration smoke.** Compose with `rgba` /  arithmetic, e.g.
  `color_scale('viridis', metric / 100.0, 1.0)` over a small literal
  table, asserting no panic and a well-formed `UInt32` result.
- **Constant folding.** Implicit — `Volatility::Immutable` plus the
  scalar fast path means a fully-literal call is folded; no explicit
  test needed unless we want to assert on the plan output. Skip for
  now.

## Dependencies

- New: `colorous = "1.0"` (Apache-2.0; no runtime dependencies; WASM-
  compatible — pure Rust with no platform code). Added at workspace
  level so other crates can reuse it if needed.

## Open Questions

None.
