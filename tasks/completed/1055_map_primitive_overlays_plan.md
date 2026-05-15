# Map Cell: Generic Primitive Overlays Plan

Addresses [issue #1055](https://github.com/madesroches/micromegas/issues/1055).

Prerequisite [#1035](https://github.com/madesroches/micromegas/issues/1035) ("keep map data in Arrow format") has landed —
`buildOverlay`/`materializeRow` live in `analytics-web-app/src/components/map/overlay.ts`
and the `Overlay { table, positions }` boundary is the canonical seam to extend.
The channel buffers introduced here (`scaleX/Y/Z`, `color`) are exactly the
pattern #1035 anticipated.

## Overview

Extend the Map cell so it can render **arbitrary primitive overlays** with column-bound
visual channels (size per axis, RGBA color). The first concrete shape after the
current `sphere` is a `box`, with the column→channel mapping general enough to accept
additional primitives later (cylinder, polyline, billboard, polygon) without
re-plumbing the renderer.

Importantly, this stays **one Map cell** with a data-driven visual mapping — not
sibling "marker cell" / "heatmap cell" / "box cell" variants. The existing cell
config grows by `shape` + `mapping` options; old notebooks keep working unchanged.

**No colormap in the cell.** Color is always RGBA, sourced as either a cell-wide
scalar or a per-row column value (string hex or u32 RGBA). Continuous-gradient
colormaps are a SQL concern — written as `CASE WHEN` quantized bands or, longer
term, a DataFusion colormap UDF (see "Future / companion UDFs"). The cell is *not*
in the business of turning a numeric metric into a color; it renders whatever
RGBA the data hands it.

## Current State

### Visual is hard-wired to sphere markers

- `MapViewer.tsx:128` allocates a single `SphereGeometry(1, 16, 16)`.
- `MapViewer.tsx:132-136` uses `MeshBasicMaterial` with `depthTest: false,
  depthWrite: false` — no transparency support; markers always render on top of the GLB.
- `MapViewer.tsx:146-178` layout pass writes `position = positions[i*3..]`,
  `scale = setScalar(markerSize)` (uniform), `color = new THREE.Color(markerColor)`
  (single cell-wide color).
- `MapViewer.tsx:743-750` currently scales `markerSize` by `extent * 0.00025`
  so a single slider feels right across maps with wildly different scales.
  **This auto-scale is removed in this PR** for both shapes: silently shrinking
  a literal `size_x = 200` in SQL by ~4 orders of magnitude is too surprising
  to keep, and a magic factor that only works near `extent ≈ 4000` is not the
  kind of thing the renderer should be guessing on the user's behalf. Sphere
  `size` becomes world units, same rule as box `scaleX/Y/Z`. See §4.

### Overlay boundary (post-#1035) only carries positions

```ts
// analytics-web-app/src/components/map/overlay.ts
export interface Overlay {
  table: Table
  positions: Float32Array   // [x0,y0,z0, x1,y1,z1, ...] in row order
}
```

`buildOverlay(table)` walks the table once, validates `x/y/z` exist and are
numeric, fills `positions`, and returns a `Result`-shaped
`{ ok: true; overlay } | { ok: false; error }` (`overlay.ts:23-69`). Selection
identity is row-index throughout. Per-row materialization is done on demand by
`materializeRow(table, rowIndex)` (`overlay.ts:71-81`).

### Cell options today

- `mapUrl: string` — GLB filename.
- `markerColor: string` — cell-wide hex color (default `#bf360c`).
- `markerSize: number` — cell-wide slider (default `10`), scaled by map extent.
- `detailTemplate: string` — Markdown template for the selection panel.

(`MapCell.tsx:159-162`.) No per-row visual binding exists. Every non-`x/y/z`
column flows through `materializeRow` into the detail panel only.

### Tests pinning the current shape

- `__tests__/MapCell.test.tsx` asserts `buildOverlay` returns
  `{ok: true, positions: [...]}` for valid inputs and `{ok: false, error}` for
  missing/non-numeric/non-finite cases; tests `materializeRow` formats every
  non-null column.
- `__tests__/EventDetailPanel.test.tsx` exercises the `row`→`$column`
  substitution.

Both files extend cleanly — `buildOverlay`'s new optional buffers don't break
the existing assertions, and `materializeRow` is unchanged.

## Design

### 1. `shape` is a cell option

```ts
shape: 'sphere' | 'box'   // extensible later: 'cylinder' | 'billboard' | ...
```

Stored on `QueryCellConfig.options.shape`. Absent ⇒ `'sphere'` (back-compat).

The renderer dispatches a different geometry per shape but reuses the
`InstancedMesh` plumbing, the layout/highlight effect split, and the
row-index-identity raycast path from #1035. One `InstancedMesh` per cell; one
shape per cell.

| Shape | Geometry | Scale semantics |
|---|---|---|
| `sphere` | `SphereGeometry(1, 16, 16)` (today) | uniform scale = `size` channel; **world units** |
| `box` | `BoxGeometry(1, 1, 1)` | non-uniform scale = `(scaleX, scaleY, scaleZ)`; **world units** |

Both shapes consume size channels in world units. The legacy `extent * 0.00025`
auto-scale is removed — see §4.

Per-row matrix composition stays on the existing `Object3D` pattern for
both shapes. `Object3D.scale` is a `Vector3`, so the box layout pass writes
`tempObject.scale.set(sx, sy, sz)` where the sphere pass writes
`tempObject.scale.setScalar(s)`. `updateMatrix()` internally calls
`Matrix4.compose(position, quaternion, scale)`, so there's no work saved by
reaching past it.

### 2. Channel mapping: `column | scalar` per channel

The cell config gains a `mapping` object. Each channel is either a column name
(`{column: 'x'}`) or a literal (`{scalar: 10}`):

```ts
// analytics-web-app/src/components/map/overlay.ts
export type ChannelBinding<T = number> =
  | { column: string }
  | { scalar: T }

export interface OverlayMapping {
  // Position (defaults to the reserved x/y/z columns)
  x?: ChannelBinding   // default { column: 'x' }
  y?: ChannelBinding   // default { column: 'y' }
  z?: ChannelBinding   // default { column: 'z' }

  // Size — semantics depend on `shape`
  size?: ChannelBinding    // sphere radius multiplier (default { scalar: 10 } ≈ legacy markerSize)
  scaleX?: ChannelBinding  // box width  (world units; default { scalar: 100 })
  scaleY?: ChannelBinding  // box depth  (world units)
  scaleZ?: ChannelBinding  // box height (world units)

  // Color — RGBA as a u32 (0xrrggbbaa). Alpha lives in the low byte; no
  // separate alpha channel.
  color?: ChannelBinding   // scalar: number (RGBA u32); column: any of:
                           //   - integer column treated bit-for-bit as RGBA u32
                           //   - string column parsed as '#rrggbb' (alpha=ff) or '#rrggbbaa'
}
```

Stored on `QueryCellConfig.options.mapping`. Absent or partial ⇒ defaults below.

Defaults preserve today's color and channel layout. Sphere visual size on
existing notebooks changes by `1 / (extent * 0.00025)` because the legacy
auto-scale is removed (§4); on a typical ~4000-unit map the change is
imperceptible. Default-mapping table:

| Shape | `mapping` defaults |
|---|---|
| `sphere` | `{ x: {column:'x'}, y: {column:'y'}, z: {column:'z'}, size: {scalar: markerSize ?? 10}, color: {scalar: rgbaFromHex(markerColor ?? '#bf360c')} }` |
| `box` | same `x/y/z`; `scaleX/scaleY/scaleZ: {scalar: 100}`; `color` same default |

The legacy `markerSize` and `markerColor` options remain readable as back-compat
seed values when no `mapping.size`/`mapping.color` is present. They are *not*
re-written on save; the editor migrates the cell to the `mapping` form the
first time the user touches a visual channel.

#### Why color carries alpha

The original issue had separate `color` (string hex), `colorValue` (numeric →
colormap), and `alpha` channels. Both extras dissolve once the cell agrees
that color is always RGBA:

- `colorValue` was the "feed this column into the LUT" channel. With colormaps
  removed (see Overview), there is no LUT in the cell — a numeric column has
  no meaningful color rendering on its own, so this channel has nothing to do.
- `alpha` was a separate 0..1 channel. With color as RGBA u32, alpha is byte
  4 of the same value. SQL emits `0xbf360c80` for half-opaque, and the editor's
  scalar picker is a combined color+alpha widget.

The lost case is "color from column X, alpha from column Y" (the issue's "color
= encounter count, alpha = recency" example). SQL handles it by combining the
two columns into one: `((color_u24 << 8) | recency_byte) AS color`. Verbose for
that one pattern, but the rest of the channel surface stays trivial.

### 3. Per-instance color: single `Uint8Array` RGBA attribute

Color (cell-wide scalar or per-row column value) is packed into one
per-instance buffer:

```ts
// Length numRows * 4. Layout: [r0,g0,b0,a0, r1,g1,b1,a1, ...]  (0..255 bytes)
colorsRGBA: Uint8Array
```

Uploaded as `InstancedBufferAttribute(colorsRGBA, 4, /* normalized */ true)`
— the `normalized: true` flag tells three.js to divide by 255 in the
shader, so the GPU sees 0..1 floats. At 100K rows the buffer is 0.4 MB
(vs 1.6 MB for a Float32 RGBA equivalent).

`MeshBasicMaterial.instanceColor` is hard-coded to RGB Float32 in three.js
core, so we can't reuse it for our RGBA. Instead we attach a custom
`instanceColorRGBA` attribute on the geometry and patch the material's
shader via `onBeforeCompile`. **Target the `#include <opaque_fragment>`
chunk directive**, not the literal `gl_FragColor = ...` line — modern
three.js (r152+) wraps the fragment output in a chunk include, and a
`String.replace` against the unexpanded line silently no-ops:

```ts
function patchInstanceColorRGBA(material: THREE.MeshBasicMaterial) {
  material.onBeforeCompile = (shader) => {
    shader.vertexShader = shader.vertexShader
      .replace('#include <common>',
               '#include <common>\nattribute vec4 instanceColorRGBA;\nvarying vec4 vInstanceColor;')
      .replace('#include <begin_vertex>',
               '#include <begin_vertex>\nvInstanceColor = instanceColorRGBA;')
    shader.fragmentShader = shader.fragmentShader
      .replace('#include <common>',
               '#include <common>\nvarying vec4 vInstanceColor;')
      // The chunk normally expands to (paraphrased):
      //   #ifdef OPAQUE\n  diffuseColor.a = 1.0;\n#endif\n
      //   gl_FragColor = vec4( outgoingLight, diffuseColor.a );
      // Preserve the OPAQUE branch (relevant for the sphere/opaque path) and
      // multiply both RGB and A by the per-instance attribute.
      .replace('#include <opaque_fragment>',
               `#ifdef OPAQUE
  diffuseColor.a = 1.0;
#endif
gl_FragColor = vec4( outgoingLight * vInstanceColor.rgb, diffuseColor.a * vInstanceColor.a );`)
  }
}
```

A single writer is the only path that touches `colorsRGBA`:

```ts
function writeRGBA(buf: Uint8Array, i: number, rgba: number) {
  const base = i * 4
  buf[base]     = (rgba >>> 24) & 0xff
  buf[base + 1] = (rgba >>> 16) & 0xff
  buf[base + 2] = (rgba >>>  8) & 0xff
  buf[base + 3] =  rgba         & 0xff
}
```

The scalar binding fills every slot with the same u32. An integer column is
read as u32 per row, with bigint coercion for 64-bit ints: Arrow JS returns
`bigint` from `col.get(i)` for any Int64 / UInt64 column (DataFusion infers
`0xbf360cff` literals as Int64 by default), but `writeRGBA`'s `>>>` operator
throws `TypeError` on bigint operands. Coerce before calling:
`const u32 = typeof v === 'bigint' ? Number(v & 0xffffffffn) : (v as number) >>> 0`.
For Int32/UInt32 columns the value is already a number; the `>>> 0` zero-fill
shift normalizes signed→unsigned (a 32-bit value with the high bit set comes
back from Arrow as a negative number, and `>>> 0` reinterprets it as UInt32
without changing the byte pattern). A string column is parsed once per row
via `rgbaFromHex('#rrggbb')` (alpha defaults to ff) or
`rgbaFromHex('#rrggbbaa')`.

**Depth and transparency, per shape.**

| Shape | `transparent` | `depthTest` | `depthWrite` |
|---|---|---|---|
| `sphere` | **false** (unchanged from today) | false | false |
| `box`    | true                              | true  | false |

The sphere material keeps its existing flags so the regression smoke
("sphere defaults visually unchanged") survives. The `OPAQUE` define is
active for the sphere path (`transparent: false`), which forces
`diffuseColor.a = 1.0` inside the shader; the alpha byte in the per-row
RGBA is multiplied into the fragment but the depth-disabled material
discards it. This matches today's behavior (markers ignore alpha; they
always paint over the GLB). Box gets `transparent: true` so the alpha
byte actually contributes to blending; depth-test is on so occlusion
against the GLB is correct. Order-of-instance artifacts where transparent
primitives overlap each other are accepted (uncommon in the headline
"grid of boxes" use case).

### 4. World-unit sizing for all shapes

`MapViewer.tsx:743-750` is removed. All size channels — sphere `size`, box
`scaleX/Y/Z` — are passed through to the per-instance matrix in world units
with no implicit map-extent multiplier.

The legacy `markerSize` slider behaved as `size * extent * 0.00025`, which
hid two distinct problems behind one magic factor:

- A SQL author writing `size_x = 200` in box mode would silently get
  `200 * extent * 0.00025` (≈0.05 on a 4000-unit map) instead of 200 — the
  kind of bug that looks like "no data" rather than "bad math."
- A sphere column binding (`mapping.size = {column: 'metric'}`) would carry
  the same surprise: a metric value the user explicitly placed in the
  channel would be rescaled by an invisible map-extent term, with no slider
  for the user to back it out.

Both go away by treating size as world units everywhere. The cost is a
one-time visual change for existing sphere notebooks on maps whose extent
≠ ~4000: markers will look bigger on a 40000-unit map, smaller on a
400-unit map. Users adjust the size scalar once per cell — same one-shot
adjustment the box path already requires.

### 5. `Overlay` becomes a bundle of channel buffers

```ts
export interface Overlay {
  table: Table
  positions: Float32Array          // length numRows * 3 (existing)

  // Per-instance RGBA, always materialized. 4 bytes/row × 100K rows = 0.4 MB,
  // small enough that the "constant vs buffer" optimization isn't worth the
  // two code paths in the renderer. Cell-wide scalar fills every slot with the
  // same u32; column binding writes per-row values.
  colorsRGBA: Uint8Array           // length numRows * 4 — [r,g,b,a, ...]

  // Size buffers are split per shape because the buffer width differs.
  // Both are absent when the relevant size channels are scalar — at 100K
  // rows the per-row scale buffer would be 1.2 MB, so the "constant vs
  // buffer" optimization still pays here.
  scales?: Float32Array            // length numRows * 3 — non-uniform scale, box only
  sizes?: Float32Array             // length numRows — uniform scale, sphere only
}
```

Size channels that resolve to a scalar are carried on a separate
`OverlayConstants` value next to the overlay so the renderer can pick
"constant" vs "per-instance" at the draw boundary:

```ts
export interface OverlayConstants {
  size: number                       // sphere fallback when overlay.sizes absent
  scale: [number, number, number]    // box fallback when overlay.scales absent
}
```

`buildOverlay` returns `{ ok: true; overlay: Overlay; constants: OverlayConstants }`
on success; `{ ok: false; error }` unchanged on failure.

Why size-only constants: at 100K rows, a numRows-length `Float32Array(3)`
buffer for box scales is 1.2 MB, which is worth avoiding when the user has
the size slider set to a single value. Color is uniformly 0.4 MB regardless,
small enough that always materializing it removes a code path without a
meaningful memory cost.

#### Builder contract

`buildOverlay(table, mapping?)` validates the bound columns:

- Position columns (resolved through `mapping.x/y/z`, default `'x'/'y'/'z'`)
  must exist and be numeric — same as today.
- For each non-position column binding, the referenced column must exist; for
  numeric channels (size/scale) it must be numeric (validated as
  `isNumericType(unwrapDictionary(field.type))`, matching the position
  pattern); for `color` it must be either an integer column (read as u32
  RGBA) or a string column (parsed as `'#rrggbb'`/`'#rrggbbaa'`), validated
  as `isIntegerType(unwrapDictionary(field.type)) || isStringType(unwrapDictionary(field.type))`.
  Caller-side `unwrapDictionary` matters for the string-color path: a
  literal `'#rrggbbaa'` in a `CASE WHEN` arrives as a dictionary-encoded
  Utf8 column in Arrow IPC, and a naked `isStringType` check would reject
  it even though `col.get(i)` resolves to a string.
- Non-finite values in any numeric channel fail the build with a row-named
  error message (same shape as the existing non-finite-x/y/z error). This
  preserves the #1035 invariant that the renderer never sees garbage —
  one NaN in a size column would otherwise poison the bounding sphere in
  the same way one NaN `x` did before #1035.
- For string-typed `color` columns, an unparseable value (e.g. `"red"`,
  `""`, or a malformed hex) fails the build with a row-named error message.

Pre-bake everything in one pass: position write, scale/size write,
`colorsRGBA` write (scalar fill, hex parse, or u32 column read). Single
iteration, typed arrays only, no JS objects per row.

**Mixed binding for box (`shape: 'box'`).** The `scales` buffer is a single
`Float32Array(numRows * 3)`, allocated iff *any* of `scaleX/Y/Z` is
column-bound. Per row, each of the three axis slots is filled from the
column value when that axis is column-bound and from the channel's scalar
literal otherwise. The headline case — variable height, fixed footprint
(`scaleZ: {column: 'h'}`, `scaleX/Y: {scalar: 100}`) — writes `100` to
`scales[i*3]` and `scales[i*3+1]` and the column value to `scales[i*3+2]`.
(The scene is Z-up — `MapViewer.tsx:702` sets `scene.up = (0,0,1)` — so
the height axis is Z, matching `scaleZ` in §2.) `constants.scale` is
*only* used by the renderer when `scales` is entirely absent (all three
axes scalar).

Two distinct paths produce a `mapping` for `buildOverlay`:

- **Cell call site (the common case).** `MapCell.tsx` always calls
  `resolveMapping(options)` first — this synthesizes a complete
  `OverlayMapping` from `options.mapping` plus legacy
  `markerColor`/`markerSize` fallbacks, then passes the resolved mapping
  into `buildOverlay(table, mapping)`. Back-compat lives here.
- **`buildOverlay`'s own default (tests and non-cell callers).** When
  `mapping` is omitted entirely, `buildOverlay` falls back to
  `defaultMappingFor('sphere')`, which uses hard-coded sphere defaults
  (`{column:'x/y/z'}` for position, `{scalar: 10}` for size,
  `{scalar: rgbaFromHex('#bf360c')}` for color). `defaultMappingFor` takes
  only `shape` — it has no access to cell options, by design.

### 6. Editor UI

`MapCellEditor` replaces the existing "Marker Color" / "Marker Size" controls
with a **Primitive** section built from channel-binding rows. One row per
visible channel; the channel set depends on `shape`.

Layout sketch:

```
SQL Query
[..............]

Primitive
  Shape:       [Sphere ▾]
  ↓ (when Sphere)
  Size:        ● scalar [── 10 ──]    ○ column [▾]
  Color:       ● scalar [picker rgba] ○ column [▾]
  ↓ (when Box)
  Scale X:     ● scalar [100]         ○ column [▾]
  Scale Y:     ● scalar [100]         ○ column [▾]
  Scale Z:     ● scalar [100]         ○ column [▾]
  Color:       ● scalar [picker rgba] ○ column [▾]

Detail Template (Markdown)
[..............]
```

Each channel row is a `scalar` vs `column` radio with the inline editor
for the chosen mode. Column dropdowns use the existing `availableColumns`
(`CellEditorProps.availableColumns`) which is already wired up from prior
runs. The color row's scalar picker is a combined color + alpha widget
(any standard `<input type="color">` + alpha slider, or a single
rgba-string input) — output is a u32 written to `mapping.color.scalar`.

Validation:

- Selecting a non-numeric column for a numeric channel ⇒ inline warning under
  the row.
- Selecting a non-integer / non-string column for `color` ⇒ inline warning.
- These are *warnings*, not blocks — execution still proceeds and
  `buildOverlay` surfaces the same error in the renderer area, mirroring
  today's SQL-error UX.

A single reusable `<ChannelBindingControl>` component renders one channel
row to keep the editor DRY across the (sphere × {size, color}) and
(box × {scaleX, scaleY, scaleZ, color}) matrix. Defined inline in
`MapCell.tsx` alongside `MapCellEditor` — it's small and has no other
caller.

### 7. Backwards compatibility

| Old cell state | New behavior |
|---|---|
| no `shape` | treated as `'sphere'` |
| no `mapping` | synthesized from `markerSize`/`markerColor` defaults |
| `markerSize: N` only | `mapping.size = {scalar: N}` at runtime |
| `markerColor: '#xxx'` only | `mapping.color = {scalar: rgbaFromHex('#xxx')}` at runtime |
| both `mapping.size` and legacy `markerSize` | `mapping.size` wins; `markerSize` ignored |

No migration step in the executor / storage layer. The editor migrates the
on-disk shape opportunistically: first time the user changes a channel, the
new `mapping` shape is written, and the legacy keys are left in place
(harmless, since they're shadowed). The detail-panel template and
data-source plumbing are unchanged.

### 8. Detail panel composes unchanged

`EventDetailPanel` receives `row: Row` from `materializeRow`. None of the new
channels affect what's substituted — every column is still available as
`$column`, including any size/color columns the user binds. Authors who want
to surface a metric in the panel write e.g. `**Frame time:** $avg_frame_ms`.

## Implementation Steps

### Phase 1 — overlay types and builder

1. **`analytics-web-app/src/components/map/overlay.ts`** (extend)
   - Add `ChannelBinding<T>`, `OverlayMapping`, `OverlayConstants` types.
   - Extend `Overlay` with `colorsRGBA: Uint8Array` (always present),
     optional `scales: Float32Array` (length numRows * 3, box-only),
     optional `sizes: Float32Array` (length numRows, sphere-only).
   - `OverlayResult` becomes
     `{ ok: true; overlay: Overlay; constants: OverlayConstants } | { ok: false; error }`.
     `constants` carries scalar size/scale fallbacks only — color is
     always materialized into `colorsRGBA`.
   - Change `buildOverlay` signature to `(table, mapping?)`. `mapping`
     absent ⇒ `defaultMappingFor('sphere')` so unchanged callers keep
     working.
   - Add helpers: `defaultMappingFor(shape)`, `rgbaFromHex(s)` (parses
     `'#rrggbb'`/`'#rrggbbaa'` to u32), and the `writeRGBA(buf, i, rgba)`
     writer from §3. `patchInstanceColorRGBA` does **not** live here —
     it depends on THREE and belongs in `MapViewer.tsx` (see Phase 2).
     `overlay.ts` stays THREE-free so the data layer remains independent
     of the renderer, matching the existing `arrow-utils.ts` precedent.
   - Single-pass row walk:
     - positions: read through `mapping.x/y/z.column` (default `'x'/'y'/'z'`),
     - size: scalar ⇒ record in `constants`; column ⇒ allocate `scales` or
       `sizes` and write,
     - colorsRGBA: every row. Scalar ⇒ fill every slot with the same u32.
       Integer column ⇒ read row value as u32. String column ⇒ parse via
       `rgbaFromHex` per row (cache the parsed value when the string is
       repeated within the column to skip redundant parses — optional
       optimization; skip in v1 unless profiling shows it matters).
   - Validation: bound-column existence; numeric type for numeric channels;
     int-or-string type for `color`; non-finite numeric ⇒ row-named error;
     unparseable color string ⇒ row-named error.

   **Also extend `analytics-web-app/src/lib/arrow-utils.ts`** with
   `export function isIntegerType(dataType: DataType): boolean` returning
   `DataType.isInt(dataType)`. The existing `isNumericType` accepts int OR
   float OR decimal, which is too permissive for the "integer column =
   bit-for-bit RGBA u32" path — a Float64 column would pass `isNumericType`
   and produce garbage colors. The helper does **not** unwrap dictionaries
   internally, matching the convention of its `isNumericType` / `isStringType`
   siblings (only `isBinaryType` unwraps internally, which is the outlier).
   Callers wrap with `unwrapDictionary` themselves — same pattern as
   `buildOverlay`'s existing position-column check at `overlay.ts:38`
   (`isNumericType(unwrapDictionary(field.type))`). This matters in practice
   for the color channel: dictionary-encoded Utf8 columns are common in Arrow
   IPC results from low-cardinality string columns (literal `'#rrggbbaa'`
   constants in a `CASE WHEN` produce exactly that), and a non-unwrapping
   check would reject them despite `col.get(i)` returning a resolved string.

### Phase 2 — renderer dispatch

2. **`analytics-web-app/src/components/map/MapViewer.tsx`**
   - Promote `InstancedMarkers` from a single-geometry component into one
     dispatched on `shape`. Keep the existing layout-effect / highlight-effect
     split; only the *geometry choice* and *matrix composition* differ.
   - **Geometry & material per shape:**
     - `sphere`: `SphereGeometry(1, 16, 16)` + `MeshBasicMaterial({
       depthTest: false, depthWrite: false })` — **no `transparent: true`**,
       keeping today's exact material flags. Markers ignore the per-row
       alpha byte (the shader patch multiplies by `vInstanceColor.a` but
       the non-transparent material discards it during composition).
     - `box`: `BoxGeometry(1, 1, 1)` + `MeshBasicMaterial({ transparent:
       true, depthTest: true, depthWrite: false })`.
     - Both materials run through `patchInstanceColorRGBA` from §3 to
       read the per-instance RGBA attribute.
   - **`instanceColorRGBA` attribute.** Allocate a runtime `Uint8Array` of
     the same length as `overlay.colorsRGBA` and wrap it in an
     `InstancedBufferAttribute(runtimeBuf, 4, /* normalized */ true)`
     attached to the geometry. The runtime buffer is what the GPU sees;
     `overlay.colorsRGBA` stays the immutable baseline (see Highlight pass
     below). Replaces the current `mesh.instanceColor` plumbing —
     `instanceColor` is unused on the new material. On `overlay` change,
     reuse the runtime buffer if length matches (otherwise reallocate),
     `set(overlay.colorsRGBA)` to refill from the new baseline, and set
     `attr.needsUpdate = true`.
   - **Layout pass updates:**
     - Read `position` from `overlay.positions[i*3..]` (unchanged).
     - Read scale from `overlay.scales[i*3..]` (box) or `overlay.sizes[i]`
       (sphere) when present; fall back to `constants.scale` /
       `constants.size`.
     - Compose the matrix via the existing `Object3D` pattern:
       `tempObject.position.set(...)`, then
       `tempObject.scale.set(sx, sy, sz)` (box) or
       `tempObject.scale.setScalar(s)` (sphere), then `updateMatrix()` and
       `setMatrixAt(i, tempObject.matrix)`. `Object3D.scale` is a
       `Vector3`, so non-uniform scales work without machinery beyond
       swapping `setScalar` for `set`.
     - No per-instance *color* write in the layout pass — `colorsRGBA` is
       baked by `buildOverlay` and uploaded once per data swap. The layout
       pass only touches matrices.
   - **Highlight pass:** unchanged in shape (still touches O(1) instances).
     Two buffers are in play:
     - `overlay.colorsRGBA` — immutable baseline produced by `buildOverlay`.
       The renderer never writes to it; it's the source of truth for the
       "normal" per-row color.
     - The geometry's `instanceColorRGBA` attribute — a runtime
       `Uint8Array` of the same length, initialized by `memcpy`'ing from
       `overlay.colorsRGBA` in the layout pass. This is the buffer the GPU
       reads; the highlight pass writes into this one.

     Highlights mutate the runtime attribute at the selected/hovered
     row's offset; `restoreNormal(i)` copies bytes from
     `overlay.colorsRGBA[i*4 .. i*4+3]` back into the runtime attribute.
     `prevHighlightRef` tracks which slots are currently highlighted (so
     the next change knows which to restore) — it does not store byte
     values, since the baseline is always available. Encapsulate
     `paintNormal(i)` / `paintHighlight(i, rgba, scaleMul)` as small
     helpers driven by the buffers so adding a future channel doesn't
     ripple. The `scaleMul` multiplier composes *on top of* the per-row
     size: sphere uses `(overlay.sizes?.[i] ?? constants.size) * scaleMul`;
     box multiplies each of the three components by `scaleMul`, sourcing
     each component from `overlay.scales[i*3+k]` when present and
     `constants.scale[k]` otherwise. Without this composition, a per-row
     sized cell would snap to the constant size on highlight.
     - Subtlety: the *highlight color* is hard-coded (`COLOR_SELECTED`,
       `COLOR_HOVERED`), but the *underlying alpha* of the row may not be
       255 (the user may have packed alpha into the color column). For
       the box path the highlight should override the alpha byte to 255
       at paint time so selection is fully visible; the next change
       restores from the baseline `overlay.colorsRGBA[i*4..i*4+3]`. No
       prior bytes are captured in the ref. (Sphere is unaffected — the
       opaque material discards alpha regardless.)
   - **Remove `effectiveMarkerSize`.** Delete the `useMemo` at
     `MapViewer.tsx:743-750` and the `markerSize`/`markerColor` props on
     `MapViewerProps`. Size and color flow through `overlay`/`constants`
     in world units; the renderer no longer scales by `mapBounds` extent.
     `mapBounds` stays in the component (still needed by
     `UnrealCameraController` for camera fit), but no marker math reads
     it.

### Phase 3 — cell wiring

3. **`analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`**
   - Read `shape` (default `'sphere'`) and `mapping` from `options`.
   - `resolveMapping(options)` synthesizes a complete `OverlayMapping` from
     stored options + legacy `markerSize`/`markerColor` fallbacks (the
     latter parsed through `rgbaFromHex`).
   - Pass `mapping` into `buildOverlay`. Key the `useMemo` on the
     underlying option fields, not on the derived `mapping` object —
     `resolveMapping(options)` returns a fresh object every render, and
     `getRendererProps` already spreads `options` into a new object on
     each call (`MapCell.tsx:504`), so a `[sourceTable, mapping]` deps
     array would re-run `buildOverlay` every render at 100K-row cost.
     Use `[sourceTable, options?.shape, options?.mapping,
     options?.markerColor, options?.markerSize]` instead.
   - Narrow `constants` alongside `overlay` from the new `OverlayResult`
     shape — extend the existing `MapCell.tsx:120` pattern:
     ```ts
     const overlay   = overlayResult?.ok ? overlayResult.overlay   : null
     const constants = overlayResult?.ok ? overlayResult.constants : null
     ```
     and gate the `<MapViewer>` render on both being non-null (they are
     produced together, so the gate is a formality — null `overlay`
     already short-circuits the render path).
   - Pass `overlay`, `constants`, and `shape` into `<MapViewer>`. Drop the
     `markerColor`/`markerSize` props from the viewer — sourced from
     constants now. (Internal-only change; the cell still reads the legacy
     options for fallback.)

4. **`MapCellEditor`** (same file)
   - Replace the existing "Marker Color" / "Marker Size" controls with
     the **Primitive** section: shape dropdown + channel-binding rows.
   - Reuse a single `<ChannelBindingControl>` component for each channel
     row.
   - For `shape === 'sphere'`, show `size` + `color` rows.
   - For `shape === 'box'`, show `scaleX/Y/Z` + `color` rows.
   - Editor writes channel changes to `mapping.<channel>` directly; the
     legacy `markerSize`/`markerColor` keys are left untouched in storage
     for back-compat reads but never re-written.

### Phase 4 — defaults & metadata

5. **`createDefaultConfig`** (in `MapCell.tsx`)
   - Seed `shape: 'sphere'`, a minimal
     `mapping: { size: {scalar: 10}, color: {scalar: 0xbf360cff} }`, and
     `detailTemplate`. Position bindings omitted ⇒ default to the
     reserved `x/y/z` columns.
   - Drop `markerColor`/`markerSize` from the new-cell seed — `MapCell.tsx`
     is the only consumer of cell config, so there is no "old parser" to
     keep happy. The back-compat path in §7 still reads these keys when
     they're present on **existing** on-disk cells; new cells just don't
     carry the ghost fields.

6. **`DEFAULT_SQL.map`** (in `notebook-utils.ts`) — keep the existing
   `SELECT NOW() as time, 0.0 as x, 0.0 as y, 0.0 as z` seed. The new
   shape stays usable with the same single-row default.

### Phase 5 — tests

7. **`analytics-web-app/src/lib/screen-renderers/cells/__tests__/MapCell.test.tsx`**
   - Existing `buildOverlay` tests stay green (default mapping path
     unchanged for the sphere defaults).
   - Add cases:
     - `buildOverlay` with `mapping.scaleX = {column: 'sx'}` writes
       per-instance scales into `overlay.scales`.
     - `buildOverlay` with `mapping.color = {column: 'c'}` over an
       Int32 column reads each value as u32 and writes the four bytes
       into `overlay.colorsRGBA[i*4 .. i*4+3]`. Use a value with the
       high bit set (e.g. `0xbf360cff`, which Arrow returns as a
       negative `number` from a signed Int32) to pin the signed→unsigned
       coercion.
     - `buildOverlay` with `mapping.color = {column: 'c'}` over an
       Int64 column (Arrow JS returns `bigint` from `col.get(i)`) reads
       each value as u32 correctly. Pins the bigint coercion step —
       without it, `writeRGBA` throws `TypeError` on the `>>>` operator.
       This is the path a literal `0xbf360cff` in a `CASE WHEN` actually
       takes by default, since DataFusion infers Int64 for integer
       literals.
     - `buildOverlay` with `mapping.color = {column: 'c'}` over a string
       column parses `'#rrggbb'` (alpha defaults to 0xff) and
       `'#rrggbbaa'` correctly, including when the column is
       dictionary-encoded Utf8 (the common case for Arrow IPC results
       from a `CASE WHEN ... THEN '#rrggbbaa'` expression). An
       unparseable string ⇒ `{ok: false, error}` naming the row.
     - `buildOverlay` with `mapping.color = {column: 'c'}` over a
       non-integer / non-string column ⇒ `{ok: false, error}`.
     - Default mapping (no `mapping`) ⇒ every RGBA slot is
       `[0xbf, 0x36, 0x0c, 0xff]` (legacy `markerColor` parsed to bytes,
       alpha 255). Pins the regression invariant.
     - Each numeric channel: non-finite value ⇒ `{ok: false, error}` naming
       the row.

8. **No new test for the shader injection** — `onBeforeCompile` runs only
    inside the WebGL context; the jest environment doesn't have one. The
    correctness gate is the manual smoke in Phase 6. (Optional pre-flight:
    instantiate the material in a unit test, invoke
    `material.onBeforeCompile` with a hand-rolled `{vertexShader,
    fragmentShader}` object containing the chunks we patch, and assert the
    patched strings contain the expected attribute declarations — catches
    the silent-no-op class of bug without needing WebGL.)

### Phase 6 — manual smoke

9. Verify the headline use case end-to-end with the local stack
    (`python3 local_test_env/ai_scripts/start_services.py`):
    - Sphere defaults: color is visually unchanged from today (default
      mapping resolves to the same `#bf360c` RGBA). Size is **expected to
      differ** on maps with extent ≠ ~4000 because the legacy auto-scale
      is removed (§4); on a typical ~4000-unit map the visual size is
      effectively unchanged. The size slider on existing notebooks may
      need a one-time adjustment.
    - Box mode: write a notebook query that emits a 32×32 grid of
      `(cell_x, cell_y, 0)` plus a quantized-color SQL expression
      (`CASE WHEN avg_frame_ms < 16 THEN 0x00ff00ff WHEN ... END AS color`),
      verify the boxes render with the expected coloring.
    - 50k+ rows in box mode renders at interactive frame rates (the
      issue's implicit acceptance criterion). Layout pass scales linearly
      with `numRows`; highlight pass stays O(1).
    - Click a box → `EventDetailPanel` shows the underlying row.
    - Transparency: a query with `0xff000080` (50% red) in the color column
      blends correctly against the GLB and against neighboring boxes (boxes
      may overlap visually if the user-supplied positions overlap;
      order-of-instance artifacts are accepted).

## Files to Modify

Frontend code:
- `analytics-web-app/src/components/map/overlay.ts` (extend `Overlay`,
  `buildOverlay`; add `defaultMappingFor`, `rgbaFromHex`, `writeRGBA`).
  Stays THREE-free.
- `analytics-web-app/src/components/map/MapViewer.tsx` (shape dispatch,
  per-instance RGBA attribute, shape-scoped transparent/depth flags;
  defines `patchInstanceColorRGBA` here — it's the only THREE-side helper
  needed by the renderer's shader injection)
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx` (mapping
  plumbing + editor; `<ChannelBindingControl>` defined inline alongside
  `MapCellEditor` — small, used only here)
- `analytics-web-app/src/lib/arrow-utils.ts` (add `isIntegerType`)

Frontend tests:
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/MapCell.test.tsx`

No changes:
- `EventDetailPanel.tsx` — `materializeRow` already surfaces every column.
- `notebook-utils.ts` `DEFAULT_MAP_DETAIL_TEMPLATE` / `DEFAULT_SQL.map` —
  templates still work with new channels.

## Trade-offs

**One shape per cell vs per-row `shape` column.** Per-cell is simpler and
covers the headline use cases ("grid of boxes" — every row is a box). Per-row
would inflate the renderer to grouping pass + N `InstancedMesh` per shape group
without a clear consumer asking for it. Issue agrees: "recommend per-cell for
v1, leave the `shape` channel as a future extension."

**Color is RGBA from SQL; no LUTs in the cell.** The original issue spec'd a
client-side colormap (`viridis`/`inferno`/...) with a domain scan and a legend
overlay. We dropped all of that: color is whatever RGBA the SQL emits.
- *Pro*: huge reduction in cell complexity (no `colormap.ts`, no LUTs, no
  `ColormapLegend.tsx`, no domain-scan pre-pass, no tri-state editor radios,
  no `colorValue` field, no precedence-with-`color` ambiguity).
- *Con*: continuous-gradient SQL is verbose. Quantized `CASE WHEN` bands
  are fine (and arguably *better* for analytics — readable thresholds);
  smooth gradients require manual bit-packing. The future-work UDFs (see
  "Future / companion UDFs") collapse the gradient case to a one-liner
  when they land.

**Per-instance color as `Uint8Array` RGBA vs `Float32Array` RGB +
separate `Float32Array` alpha.** RGBA Uint8 normalized is 4× smaller (0.4 MB
vs 1.6 MB at 100K rows), one buffer instead of two, and the alpha lives
adjacent to the RGB in memory so a single `writeRGBA` per row covers both
channels at every source (scalar, integer column, hex column). The cost is
a custom shader patch via `onBeforeCompile` — we can't reuse three.js's
built-in `instanceColor` plumbing since that's hard-coded vec3 Float32 —
and the bytes are interpreted as linear in the shader despite arbitrary
user-supplied semantic intent. We accept the color-management imprecision
because these are false colors for analytics.

**Replace `#include <opaque_fragment>` (not the expanded `gl_FragColor`
line).** Modern three.js (r152+) wraps the final fragment output in a
chunk include. Patching the literal `gl_FragColor = vec4( outgoingLight,
diffuseColor.a );` against `shader.fragmentShader` silently no-ops because
that line lives in the chunk, not the unexpanded source. Replacing the
`#include` directive itself is the supported extension point.

**`onBeforeCompile` vs custom `ShaderMaterial`.** `onBeforeCompile` keeps
`MeshBasicMaterial`'s lighting/uniform handling intact and adds one
attribute + two shader patches. A custom `ShaderMaterial` is ~60 lines and
re-implements the same trivial fragment output. The injection is locally
weird but cheap to read and removes the "why do we have a custom material"
question from the codebase.

**`depthTest`/`transparent` per shape, not per-cell.** Markers today rely
on depth-disabled rendering to stay visible regardless of where they sit
relative to the GLB (the existing `MeshBasicMaterial` flags at
`MapViewer.tsx:132`). For aggregated boxes, the user expectation is the
opposite — a box hidden behind a wall *should* be hidden, and per-row
alpha must actually contribute to blending. Same renderer, both behaviors,
via per-shape material config. Sphere material is kept byte-identical to
today (no `transparent: true`), so the regression smoke survives.
Long-term these could be exposed as per-shape options (`depthMode`,
`blendMode`); deferred until a concrete request appears.

**Mapping shape: `{column} | {scalar}` vs flat string + ad-hoc convention.**
The union form is wordier in config (`{scalar: 10}` vs `10`) but
self-describing: `{column: 'avg_frame_ms'}` and `{scalar: 100}` never
collide on a column literally named `"100"`. The editor can write the right
shape based on which radio is selected; the renderer never has to sniff.
Precedent in the codebase: variable values use the
`string | Record<string, string>` union with the `mcol:` URL prefix
(`notebook-types.ts:52-90`) for the same self-describing reason.

**Channel buffers as separate fields vs one interleaved buffer.** Separate
`scales`/`sizes` + `colorsRGBA` allow size channels to be absent (scalar)
without wasting space. An interleaved buffer would force every channel to
be materialized to numRows. The per-instance GPU upload is the same either
way (multiple `InstancedBufferAttribute`s vs one with offsets).

**Color column accepts both integer and string types.** A more restrictive
design (string-only or integer-only) would be one fewer dispatch path in
`buildOverlay`. Both types come up naturally in SQL: integers from bitwise
construction (`(r << 24) | (g << 16) | (b << 8) | a`), strings from string
concatenation or literal `'#rrggbbaa'` constants. Forcing one would force
users into the other's idioms unnecessarily. Type dispatch is one
`isIntegerType()` / `isStringType()` check, run once at build time per
column.

## Documentation

`mkdocs/docs/web-app/notebooks/cell-types.md` — extend the Map section:

- Replace the "Options" table to add `shape` and `mapping`.
- Add a "Visual channels" sub-section explaining each channel and the
  `column | scalar` binding shape.
- Add a "Color encoding" sub-section: scalar is RGBA u32 (`0xrrggbbaa`);
  column can be integer (read as u32) or string (`'#rrggbb'` /
  `'#rrggbbaa'`). Document the alpha byte is the low byte.
- Add a worked example: 32×32 grid of boxes colored by `avg_frame_ms`
  with `CASE WHEN ... AS color` SQL and a screenshot.
- Add a "Sizing" sub-section: all size channels (`size`, `scaleX/Y/Z`) are
  in world units. Note that the legacy `markerSize` slider's implicit
  map-extent auto-scale is gone; on an existing notebook where markers
  used to look right, the slider value may need a one-time adjustment
  proportional to the map's extent.
- Update the "Required columns" note: `x`, `y`, `z` remain the *default*
  position bindings; `mapping.x/y/z` overrides can rename them.
- Add a callout pointing to the future UDFs (see "Future / companion UDFs"
  in this plan) so users know the smoother gradient ergonomics are
  planned.

The detail-template section and map catalog setup are unchanged.

## Testing Strategy

- **Unit** (`MapCell.test.tsx`): cover every channel × column-binding path
  through `buildOverlay`, including type-mismatch and non-finite errors,
  plus the default-mapping regression invariant.
- **Component**: editor reuses validated controls via
  `<ChannelBindingControl>`; no new component-level tests unless the radio
  behavior gets non-trivial.
- **Pre-flight unit test** (optional, Phase 5 step 8): invoke
  `material.onBeforeCompile` on a synthetic shader object containing the
  patched chunks; assert the resulting strings contain
  `instanceColorRGBA` and the expanded `gl_FragColor` line. Catches the
  silent-no-op class of bug pre-runtime.
- **Manual smoke** (Phase 6): box mode with 50k+ rows, quantized-band
  colors via SQL, transparency, depth occlusion against the GLB, detail
  panel.
- **Regression**: existing notebooks (sphere defaults, no `mapping`)
  keep their color (default mapping resolves to the same `#bf360c`).
  Sphere visual size will differ on maps with extent ≠ ~4000 because
  the `extent * 0.00025` auto-scale is removed (§4) — this is an
  accepted, one-time visual change, not a bug. Color regression is
  pinned by the default-mapping unit test (Phase 5 step 7); the size
  change is documented for users in the cell-types docs.

## Out of Scope

These were explicitly punted in the issue and stay punted here:

- Per-row `shape` column (mixed primitives in one query).
- Multi-layer Map cell (multiple SQL queries layered).
- Client-side colormaps / LUTs / legend overlay — pushed to SQL with
  optional UDFs in a follow-up (see "Future / companion UDFs").
- Client-side aggregation / spatial indexing — assume the SQL author wrote
  `GROUP BY cell_x, cell_y` themselves.
- Animated time playback / LOD / marker clustering.
- Cylinder, polyline, polygon, billboard primitives — the `shape` enum is
  extensible but no implementation in this PR.

## Future / companion UDFs

The "color is RGBA" stance pushes color math to SQL. DataFusion UDFs
would close the ergonomics gap without re-introducing client-side LUTs.
None of these are in this PR; they're a separate, smaller follow-up.

**Color construction**
- `rgba(r, g, b, a) -> UInt32` — pack four 0..255 ints into RGBA u32.
- `rgb(r, g, b) -> UInt32` — same with alpha implicitly 0xff.
- `hex_to_rgba(s) -> UInt32` — parse `'#rrggbb'` / `'#rrggbbaa'`.
- `rgba_with_alpha(c, a) -> UInt32` — replace the alpha byte; useful when
  the caller wants a colormap-derived RGB with a metric-driven alpha.

**Color interpolation**
- `lerp_color(c1, c2, t) -> UInt32` — linear interpolation between two
  RGBA u32s with `t` in `[0, 1]`. Component-wise; alpha included.
- `mix_colors(c1, c2, w1, w2) -> UInt32` — weighted blend (w1+w2=1
  convention or normalized).

**Continuous colormaps**
- `viridis(t)`, `inferno(t)`, `turbo(t)`, `plasma(t)`, `rdbu(t)` — each
  returns u32 RGBA from `t` in `[0, 1]`. Bundled LUTs server-side
  (matplotlib / ColorBrewer).
- `colormap(name, t)` — unified entry point; `name` is one of the above
  string literals. Lets users centralize the choice.

**Domain helpers**
- `normalize(v, lo, hi) -> Float64` — `(v - lo) / (hi - lo)` clamped to
  `[0, 1]`. Trivially expressible with arithmetic, but explicit name is
  more readable in queries.

**Color manipulation**
- `darken(c, amount) -> UInt32`, `lighten(c, amount) -> UInt32` — adjust
  perceived lightness in HSL space.
- `with_alpha(c, a)` — alias for `rgba_with_alpha`; mentioned here for
  the natural "give me this color but X% opaque" pattern.

When these land, the headline gradient case collapses from a four-branch
`CASE WHEN` to:

```sql
SELECT cell_x AS x, cell_y AS y, 0 AS z,
       viridis(normalize(avg_frame_ms, 0, 100)) AS color
FROM frame_timing
GROUP BY cell_x, cell_y
```

That's the eventual ergonomic story. Until the UDFs ship, the `CASE WHEN`
form is acceptable.

## Open Questions

None blocking implementation. Worth surfacing during review:

1. **Stroked / outlined boxes.** The issue doesn't mention them, but a
   1-px outline around each box is the visual norm for grid-style
   heatmaps and makes adjacent cells separable. The trivial way is a
   second `InstancedMesh` of `EdgesGeometry(box)` rendered with
   `LineBasicMaterial`. Defer until the first reviewer asks.
2. **Companion UDFs in this PR vs a follow-up.** Splitting them out
   keeps the Rust/DataFusion work separable from the React/three.js
   work in this issue. Confirm with reviewers that the split is OK
   before merging.
