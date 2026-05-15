# Color UDFs Plan

Issue: [#1062](https://github.com/madesroches/micromegas/issues/1062)

## Overview

Add two scalar UDFs that let SQL queries build RGBA colors for the map cell's
`color` channel without ad-hoc `CASE WHEN` bit-twiddling:

- `rgba(r, g, b, a) -> UInt32` — pack four `[0.0, 1.0]` floats into a packed
  RGBA `u32` (each channel scaled to `0..255`).
- `lerp_color(c1, c2, t) -> UInt32` — component-wise linear interpolation
  between two packed RGBA `u32`s with `t` in `[0.0, 1.0]`. Alpha is interpolated
  along with RGB.

These are the first installment of the colormap UDFs anticipated by the map
primitive overlays plan
(`tasks/completed/1055_map_primitive_overlays_plan.md`, lines 23–27).

## Current State

- **Packing convention.** The map cell consumes packed RGBA `u32`s in
  `0xRRGGBBAA` byte order. `MapViewer.tsx:293-296` decodes a packed scalar as
  `r = (c >>> 24) & 0xff`, `g = (c >>> 16) & 0xff`, `b = (c >>> 8) & 0xff`,
  `a = c & 0xff`. Any new UDF must produce the same layout to interoperate.
- **UDF home.** Generic, WASM-compatible scalar UDFs live in
  `rust/datafusion-extensions/src/` and are registered in
  `register_extension_udfs()` in
  `rust/datafusion-extensions/src/lib.rs:39-70`. The crate is described as
  "WASM-compatible DataFusion UDF extensions (JSONB, histogram) for
  micromegas" — color UDFs are a natural new neighbor.
- **Implementation pattern.** Two patterns coexist in this crate: the
  histogram UDFs use the `create_udf` helper (function-pointer style), while
  the JSONB and properties UDFs define a `struct` implementing
  `ScalarUDFImpl` with a `make_*_udf()` constructor returning a `ScalarUDF`.
  The color UDFs follow the struct pattern (it gives explicit control over
  the signature and lets us downcast each input array). Every struct-based
  UDF in this crate (JSONB, properties) derives
  `#[derive(Debug, PartialEq, Eq, Hash)]` so the default `equals`/`hash_value`
  on `ScalarUDFImpl` work without a hand-written impl. See
  `rust/datafusion-extensions/src/jsonb/array_length.rs` for a concise
  numeric example: explicit `Signature`, `return_type`, `invoke_with_args`
  building an Arrow array directly.
- **Test convention.** Per `CLAUDE.md`, tests live in the crate's `tests/`
  folder, not next to the lib. Existing example:
  `rust/datafusion-extensions/tests/jsonb_array_length_tests.rs` builds a
  `SessionContext`, calls `register_extension_udfs(&ctx)`, runs SQL, and
  asserts on the resulting Arrow array.
- **Documentation.** The SQL functions reference is
  `mkdocs/docs/query-guide/functions-reference.md`. Scalar functions are
  grouped by topic (JSON/JSONB, Data Access, Property, Histogram). Color
  functions warrant a new subsection.

## Design

### API conventions (locked in by this plan)

These are the conventions every color UDF in this crate must honour. Once
external SQL exists in the wild, they are effectively frozen — document them
in the `color/mod.rs` module doc-comment so future UDFs reuse one source of
truth.

- **Packing.** Colors are packed `u32` in `0xRRGGBBAA` byte order: byte 0
  (high byte) is red, byte 3 (low byte) is alpha. Matches the map cell's
  decode in `MapViewer.tsx:293-296`.
- **Component range.** Float inputs/outputs are in `[0.0, 1.0]`. Out-of-range
  values are clamped at the byte boundary, not rejected.
- **Alpha is straight, not premultiplied.** No special casing of the alpha
  channel — it interpolates and quantizes like RGB.
- **Color space is sRGB-encoded 8-bit.** Lerps and other ops happen directly
  on sRGB byte values, which is what the GPU consumes and what
  HLSL/Cg-style 8-bit color code does. Future perceptual-space variants
  (e.g., `lerp_oklab`) must use an explicit suffix; unsuffixed names always
  mean sRGB.

### Future-extension naming reservations

Not built now, but the names are reserved so the API stays coherent as it
grows. Recording them here keeps the next person from picking conflicting
names.

- **Constructors by format:** `rgba(r,g,b,a)` (this plan), `rgb(r,g,b)`
  (alpha=1), `hsla(h,s,l,a)`, `hsva(...)`, `color_from_hex('#bf360cff')`.
- **Operations on packed colors:** `<op>_color` suffix —
  `lerp_color` (this plan), and (future) `mix_color` (alias?),
  `blend_color` (over-compositing), `tint_color`, `saturate_color`.
- **Color-space-specific operations:** `<op>_<space>` suffix —
  `lerp_oklab`, `lerp_hsl`, etc.
- **Component accessors** (`color_r/g/b/a(c) -> Float64`) are deliberately
  out of scope for this issue — colormap-band SQL builds colors up rather
  than tearing them down. Worth a follow-up if anyone needs to manipulate
  existing color columns.

### Module layout

Add a `color` submodule to the extensions crate, mirroring the existing
`jsonb` / `histogram` / `properties` layout. Two functions warrant a folder
because more color UDFs are anticipated (e.g., `colormap_viridis`,
`color_from_hex`); putting them in their own module keeps `lib.rs` tidy and
avoids future churn.

```
rust/datafusion-extensions/src/
  color/
    mod.rs          # shared helpers (pack, unpack, clamp) + submodule decls
    rgba.rs         # rgba(r,g,b,a) -> UInt32
    lerp_color.rs   # lerp_color(c1,c2,t) -> UInt32
```

### Packing helpers (in `color/mod.rs`)

Centralize the byte-order convention so future color UDFs reuse it:

```rust
#[inline]
pub fn pack_rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    ((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | (a as u32)
}

#[inline]
pub fn unpack_rgba(c: u32) -> (u8, u8, u8, u8) {
    (
        ((c >> 24) & 0xff) as u8,
        ((c >> 16) & 0xff) as u8,
        ((c >> 8)  & 0xff) as u8,
        ( c        & 0xff) as u8,
    )
}

/// Quantize a normalized float to 0..=255 with round-half-up.
/// Clamps the scaled value (not the input) so the output invariant holds
/// regardless of input range. ±∞ saturate via the `clamp`; NaN saturates to
/// 0 via Rust's saturating `f64 as u8` cast (the clamp itself propagates
/// NaN, so the cast is what makes NaN inputs safe).
#[inline]
pub fn float_to_byte(f: f64) -> u8 {
    (f * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// Round-half-up an already-in-`[0,255]` lerp result to a `u8`. Shared with
/// `lerp_color` so both UDFs use the same tie-breaking rule.
#[inline]
pub fn round_to_byte(f: f64) -> u8 {
    (f + 0.5).clamp(0.0, 255.0) as u8
}
```

### `rgba(r, g, b, a) -> UInt32`

- **Signature:** `Signature::exact(vec![Float64; 4], Volatility::Immutable)`.
  DataFusion will coerce `Float32`, `Int64`, etc. to `Float64`, so users can
  write `rgba(1, 0.5, 0, 1)` without explicit casts.
- **Return type:** `UInt32`.
- **Behavior:**
  - Iterate row-wise over the four `Float64Array`s.
  - For each row, if any input is null → result is null.
  - Otherwise quantize each component via `float_to_byte` (which scales then
    clamps to `0..=255`). Pack with `pack_rgba`.
- **Out-of-range inputs are clamped, not rejected.** Friendlier for SQL
  expressions that produce values slightly outside `[0,1]` due to floating
  rounding (e.g., `metric / max_metric`). NaN and ±∞ are clamped to the
  output range by `float_to_byte`.

### `lerp_color(c1, c2, t) -> UInt32`

- **Signature:** `Signature::exact(vec![UInt32, UInt32, Float64], Volatility::Immutable)`.
- **Return type:** `UInt32`.
- **Behavior:**
  - Iterate row-wise over the two `UInt32Array`s and one `Float64Array`.
  - If any input is null → result is null.
  - Clamp `t` to `[0.0, 1.0]`.
  - Unpack both colors into four `(u8, u8, u8, u8)` tuples, lerp each channel
    as `f64` (`a + (b - a) * t`), quantize via the shared `round_to_byte`
    helper (round-half-up, same tie-breaking as `rgba`'s `float_to_byte`),
    and repack.
  - Alpha is treated the same as RGB (no premultiplication, no special-casing).
- **Caller note (literal colors).** Unlike `Float64`, DataFusion 52.5 does **not**
  coerce `Int64` (or `Binary`, which is how hex literals like `0xff000000` parse)
  into `UInt32` under `Signature::exact` — verified empirically. Callers passing
  literal colors must either:
  - Construct them via `rgba(...)` (returns `UInt32` natively), or
  - Wrap them in `CAST(<literal> AS INT UNSIGNED)`.

  Column references that are already `UInt32` (e.g. a stored color column) work
  without ceremony. This needs to be called out explicitly in the docs page.

### Why a single t (not per-channel)

The issue specifies a scalar `t`. This matches the typical colormap-band use
case (`lerp_color(low, high, (value - lo) / (hi - lo))`). Per-channel lerp
would invite confusion with multiplicative tints; leave that to a future
`mix_color` if needed.

### Registration

Append to `register_extension_udfs` in `rust/datafusion-extensions/src/lib.rs`:

```rust
ctx.register_udf(make_rgba_udf());
ctx.register_udf(make_lerp_color_udf());
```

With corresponding `use color::{rgba::make_rgba_udf, lerp_color::make_lerp_color_udf};`
at the top.

## Implementation Steps

1. Add `pub mod color;` to `rust/datafusion-extensions/src/lib.rs`.
2. Create `rust/datafusion-extensions/src/color/mod.rs` with the four
   helpers (`pack_rgba`, `unpack_rgba`, `float_to_byte`, `round_to_byte`)
   and `pub mod rgba; pub mod lerp_color;`. Lead with a module doc-comment
   that states the four locked-in API conventions (packing, component
   range, straight alpha, sRGB color space) — this is the canonical
   reference for future color UDFs.
3. Implement `rust/datafusion-extensions/src/color/rgba.rs` —
   `RgbaUdf` struct, `ScalarUDFImpl` impl, `make_rgba_udf()`.
4. Implement `rust/datafusion-extensions/src/color/lerp_color.rs` —
   `LerpColorUdf` struct, `ScalarUDFImpl` impl, `make_lerp_color_udf()`.
5. Register both UDFs in `register_extension_udfs()` (`lib.rs`).
6. Add `rust/datafusion-extensions/tests/color_tests.rs` (see Testing
   Strategy).
7. Add a "Color Functions" subsection to
   `mkdocs/docs/query-guide/functions-reference.md`, placed after "Histogram
   Functions". Match the existing entry style (Syntax / Parameters / Returns
   / Examples).
8. Update the `description` field in `rust/datafusion-extensions/Cargo.toml`
   to mention color (and fill in the already-omitted properties module),
   e.g. `"WASM-compatible DataFusion UDF extensions (JSONB, histogram,
   properties, color) for micromegas"`.
9. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and
   `cargo test -p micromegas-datafusion-extensions`.

## Files to Modify

- `rust/datafusion-extensions/Cargo.toml` — add "color" to crate
  description.
- `rust/datafusion-extensions/src/lib.rs` — add module, register UDFs.
- `rust/datafusion-extensions/src/color/mod.rs` (new).
- `rust/datafusion-extensions/src/color/rgba.rs` (new).
- `rust/datafusion-extensions/src/color/lerp_color.rs` (new).
- `rust/datafusion-extensions/tests/color_tests.rs` (new).
- `mkdocs/docs/query-guide/functions-reference.md` — add Color Functions
  subsection.

## Trade-offs

- **`color/` subdirectory vs. a single `color.rs`.** Two small functions
  could live in one file, but other UDF categories in this crate use a
  subdirectory each, and more color UDFs are explicitly anticipated. Matching
  the existing convention now avoids a rename later.
- **Float `[0,1]` API vs. int `0..=255`.** Decided in the issue thread:
  floats compose more cleanly with SQL expressions that produce normalized
  values (`metric / max_metric`), at the cost of hardcoded colors being less
  readable than the CSS `rgba(255,128,0,255)` form. A future `color_from_hex`
  UDF can cover the hardcoded-palette use case.
- **Clamp vs. error on out-of-range floats.** Clamping. SQL inputs are often
  derived from arithmetic that may drift slightly past `[0,1]`; an error on
  every such row would be more surprising than the silent clamp.
- **Clamp after scaling, not before.** Encodes the actual invariant (output
  is `0..=255`). ±∞ saturate to `0` / `255` via the clamp directly; NaN
  propagates through the clamp (per `f64::clamp` semantics) and is folded
  to `0` by Rust's saturating `f64 as u8` cast. Both safety nets are load-
  bearing — the comment on `float_to_byte` calls this out so future readers
  don't strip either one.
- **Round-to-nearest vs. truncation when quantizing.** Round (`* 255 + 0.5`).
  Matches what users expect when reading `1.0 → 255` and avoids the
  truncation artifact where `1.0 * 255 = 255` but `0.999 * 255 = 254`.
- **Signature::exact vs. variadic.** Exact. The argument counts are fixed and
  the arg types are specific; DataFusion's implicit numeric coercion handles
  callers that pass `Int64`/`Float32` literals.
- **Single UDF file (`color.rs`) with both impls.** Considered but rejected —
  two siblings under `color/` matches the rest of the crate and keeps file
  sizes small.

## Testing Strategy

`rust/datafusion-extensions/tests/color_tests.rs`, following the
`jsonb_array_length_tests.rs` pattern (build a `SessionContext`, register
extension UDFs, run SQL, assert).

Coverage:

- **rgba — basics:** `rgba(1, 0, 0, 1)` → `0xff0000ff`; `rgba(0, 0, 0, 1)` →
  `0x000000ff`; `rgba(0, 0, 0, 0)` → `0x00000000`.
- **rgba — clamping:** `rgba(2.0, -1.0, 0.5, 1.0)` → `0xff0080ff` (or
  whatever round-to-nearest gives for `0.5`).
- **rgba — quantization:** `rgba(0.5, 0.5, 0.5, 1.0)` → byte value `128`
  (round, not truncate).
- **rgba — nulls:** if any of the four inputs is NULL, the row's result is
  NULL.
- **lerp_color — endpoints:** with `c1`/`c2` built via `rgba(...)` or
  `CAST(... AS INT UNSIGNED)`, `lerp_color(c1, c2, 0.0) == c1` and
  `lerp_color(c1, c2, 1.0) == c2`. (Bare `UInt32` literals do not coerce —
  see the caller note in the `lerp_color` design section.)
- **lerp_color — midpoint:** `lerp_color(rgba(1, 0, 0, 0), rgba(0, 1, 0, 0), 0.5)`
  → `0x80800000` (round-half-up via the shared `round_to_byte` helper: `127.5
  → 128` on both R and G channels). Equivalent form:
  `lerp_color(CAST(4278190080 AS INT UNSIGNED), CAST(16711680 AS INT UNSIGNED), 0.5)`.
- **lerp_color — t clamping:** `t = -0.5` behaves like `t = 0`, `t = 1.5`
  like `t = 1`.
- **lerp_color — alpha interpolation:** confirm alpha channel interpolates
  alongside RGB.
- **lerp_color — nulls:** any null input → null result.
- **Integration smoke:** a SQL expression combining the two,
  e.g., `lerp_color(rgba(1,0,0,1), rgba(0,0,1,1), 0.5)`.
- **Coercion regression guard:** an explicit negative test asserting that
  `lerp_color(0xff000000, 0x00ff0000, 0.5)` (or `lerp_color(4278190080, 65280, 0.5)`)
  fails at planning time with a coercion error — pins the documented caller
  contract so a future signature change doesn't silently break it.

## Documentation

Add a new `#### Color Functions` group to
`mkdocs/docs/query-guide/functions-reference.md`, placed after `#### Histogram
Functions`. One subsection per UDF, matching the existing template (Syntax,
Parameters, Returns, Examples). The `lerp_color` subsection must explicitly
note that literal color inputs must be constructed via `rgba(...)` or cast with
`CAST(<literal> AS INT UNSIGNED)` — bare hex/integer literals do not satisfy
the `UInt32` signature. Examples should reference the map-cell `color` channel
as the primary motivating use case, e.g.:

```sql
-- Hot/cold gradient over a metric, with full alpha.
-- `t` is clamped internally, so out-of-range ratios safely saturate.
SELECT x, y, z,
       lerp_color(rgba(0, 0.5, 1, 1),       -- cool
                  rgba(1, 0.2, 0, 1),       -- hot
                  value / 100.0) AS color
FROM my_events;
```

No update needed for `CLAUDE.md` or `AI_GUIDELINES.md`.

## Open Questions

- **Naming.** `rgba` is short and memorable but generic enough to collide
  with future variants (e.g., a hex-string form). If a collision feels
  likely, `pack_rgba` is an alternative; recommend keeping `rgba` for
  ergonomics.
