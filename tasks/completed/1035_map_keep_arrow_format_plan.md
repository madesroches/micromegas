# Map Cell: Keep Data in Arrow Format Plan

Addresses [issue #1035](https://github.com/madesroches/micromegas/issues/1035).

## Overview

Refactor the Map cell so that the SQL result stays as an Arrow `Table`
through the render path. Today the cell calls `arrowTableToMapEvents`
once per re-execution, allocating a `MapEvent[]` of length `numRows`
plus a `Record<string, string>` for *every* row's columns. That pass
is O(N · C) in allocations and dominates render cost long before the
GPU does. The fix is to walk the table once into a small typed-array
struct (positions) and let downstream consumers read columns by name
— same shape as `buildFlameIndex` in
`FlameGraphCell.tsx:107-230`. Identity stays in row-index space
end-to-end: marker index = row index, `e.instanceId` from the
raycaster *is* the selectable row, and the only place that still
materializes a JS row is `EventDetailPanel`, which runs once per
click.

The `MapEvent` wrapper (`id`/`x`/`y`/`z`/`time`/`row`) collapses to
just `Row = Record<string, string>` — a single row of the table,
formatted to strings. Selection identity moves to the row index,
so `id` is no longer needed; `x`/`y`/`z`/`time` were never read by
the panel (only `row` was). `EventDetailPanel` becomes
`{ row, template, ... }` directly.

## Current State

### Where materialization happens

`MapCell.tsx:40-81` —
[`arrowTableToMapEvents(table)`](../analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx)
iterates `table.numRows` and for each row:
- calls `table.get(i)` (returns a JS proxy object backed by the row),
- parses `x`/`y`/`z` via `parseFloat(String(...))`,
- walks `table.schema.fields` and copies every non-null column into a
  fresh `Record<string, string>`, formatting timestamps through
  `formatArrowValue`,
- pushes a `MapEvent` with `id`, optional `time: Date`, the three
  coordinates, and the row map.

`MapCell` then `useMemo`s the array on the `sourceTable` reference
(`MapCell.tsx:166-169`) and passes it to `MapViewer`.

### What downstream code does with `MapEvent[]`

`MapViewer.tsx:117-234` — `InstancedMarkers` consumes
`events: MapEvent[]`:
- a `useEffect` walks the array every time any of `events`,
  `selectedIndex`, `hoveredIndex`, `markerColor`, `markerSize` change,
- for each event it sets a translation/scale matrix on the
  `InstancedMesh` and a per-instance color on `instanceColor`,
- raycast hit → `instanceId` → `events[instanceId]` is the clicked
  event (`MapViewer.tsx:189-203`).

`MapCell.tsx:203-206` — on click, the cell calls
`onSelectionChangeRef.current?.(event.row)` so the row is published
to `cellSelections` for downstream cells.

`EventDetailPanel.tsx:46-81` — renders a Markdown template by spreading
`event.row` over the notebook variables and feeding the result through
`substituteMacros` + `react-markdown`. The only field it reads off the
typed shape is `event.row`; `time`/`x`/`y`/`z` are addressed as
`$time`/`$x`/`$y`/`$z` and resolve from `row`.

`EventDetailPanel.tsx:55-60` — `useMemo` over the *event reference*
plus the template/variables. Today every re-execution allocates a new
event object even when the selection didn't change, but that's
incidental to the materialization itself.

### What does not exist (yet)

The issue mentions `HeatmapLayer` and `transformEvents` as consumers
to refactor. Neither is in the tree today — the heatmap layer was
removed in commit `ec026d85d` (#1045), and there is no
`transformEvents` helper. The refactor must therefore *enable* those
to read columns directly when they come back, but there's no existing
code to migrate. The design below builds an `Arrow Table`-shaped
boundary so a future heatmap layer can walk `overlay.positions` (or
column vectors) once without forcing the cell back into a
`MapEvent[]`.

### Tests pinning the current behavior

`__tests__/MapCell.test.tsx` — six unit tests on
`arrowTableToMapEvents`:
1. stores every non-null column as a string,
2. omits null-valued columns,
3. formats timestamp columns as RFC3339,
4. leaves `time` undefined when no `time` column exists,
5. skips NaN x/y/z rows,
6. derives `id` from `process_id` or "unknown".

These all assert against materialized `MapEvent` objects produced by
the standalone helper. After the rewrite the helper is gone, so these
tests are reframed to assert on the new boundary
(`buildOverlay` + a `materializeRow` helper).

`__tests__/EventDetailPanel.test.tsx` (current) — builds a `MapEvent`
directly with a `row` field and renders the panel. After the
`MapEvent` → `Row` collapse the panel takes a `row` prop directly,
so the test's `buildEvent` helper becomes `buildRow` returning a
plain `Record<string, string>` and the `<EventDetailPanel
event={...}>` calls become `<EventDetailPanel row={...}>`. The
substitution assertions themselves (the `$x`/`$time`/`$from`/`$to`
expectations) stay the same — the panel still substitutes from the
row dict.

## Design

### Boundary: `Overlay` (value) and `OverlayResult` (value-or-error)

Define a lightweight struct analogous to `FlameIndex` — bundles the
table with the per-instance buffers and schema-derived facts that the
renderer needs. The name leaves room for #1055's channel expansion
(size, color, alpha, yaw buffers grow this struct rather than
replacing it).

Unlike `FlameIndex` (which carries an optional `error` alongside
half-built data), `buildOverlay` returns a Rust-`Result`-style
discriminated union: a build is either fully successful or it
returned an error. This mirrors the existing chart helpers in
`arrow-utils.ts` (`extractChartData`, `extractMultiSeriesChartData`
— both return `{ ok: true; ... } | { ok: false; error }`). Why not the optional-
`error` shape: the success-path `positions` array is
allocated *during* the row walk, so a row-level error has to either
"return early with a partially populated array" (the original draft
of this plan) or duplicate the allocation. The union form makes the
states syntactically mutually exclusive — the type system refuses
any code that reads `positions` from an error result.

```ts
export interface Overlay {
  table: Table
  /** Flat [x0,y0,z0, x1,y1,z1, ...] in row order. Length = numRows * 3.
      All values are finite (rows with non-finite x/y/z fail the build
      with `OverlayResult.ok = false`, so the consumer never sees a
      partially populated buffer). */
  positions: Float32Array
}

export type OverlayResult =
  | { ok: true; overlay: Overlay }
  | { ok: false; error: string }
```

`buildOverlay(table): OverlayResult` validates the schema, then walks
rows once:
- look up `x`/`y`/`z` columns via `table.getChild('x')` etc.; if any
  are missing return `{ ok: false, error: 'Missing required columns:
  ...' }`,
- check that each column's `DataType` is numeric (`isNumericType` from
  `arrow-utils.ts`); if not, return `{ ok: false, error: '...' }`
  with a type-mismatch message,
- allocate `positions = new Float32Array(numRows * 3)`,
- for `i = 0 .. numRows-1`, read `Number(xCol.get(i) ?? NaN)` (etc.)
  and write into the three slots. If any of the three is non-finite
  (`!Number.isFinite`), return `{ ok: false, error: "Row ${i}:
  non-finite coordinate (x=${x}, y=${y}, z=${z}). Filter NaN/null
  values in your SQL." }`. The partial `positions` buffer is dropped
  on the failed-return path and never reaches a caller,
- return `{ ok: true, overlay: { table, positions } }`.

One pass, one array (`positions`, plus the schema check), no
per-row JS objects. Identity is the Arrow row index throughout —
marker index = row index, no translation needed.

### NaN / non-finite handling

x/y/z must be finite. The current `arrowTableToMapEvents` silently
skips rows whose coords are NaN, which preserves row identity but
hides bad data from the SQL author. We instead push that
responsibility to the SQL author: `buildOverlay` walks the columns
once and, on encountering any non-finite value, returns
`{ ok: false, error }` naming the offending row. The cell renders
that error message inline (same path as the missing-column case).

Why detect rather than pass through: a single NaN row poisons the
`InstancedMesh` bounding sphere — `InstancedMesh.computeBoundingSphere`
applies every instance matrix to the geometry sphere and `Sphere.union`
does not skip NaN-centered spheres (radius stays finite). The
poisoned `boundingSphere.center` makes `Ray.intersectsSphere` return
false (NaN comparisons), and `InstancedMesh.raycast` bails on the
bounding-sphere gate *before* the per-instance loop runs. The
practical effect: one NaN row makes *every* marker unclickable. A
build-time error is a much friendlier failure mode than "no
markers respond to clicks anywhere on the map."

This is a visible-behavior change from the current code (silent
drop → loud error). The test (`MapCell.test.tsx:75-84`) that pins
the silent-drop behavior is reframed to assert
`{ ok: false, error }` with a row-level message instead — see
Phase 4.

### Row materialization (on demand)

Selection still produces a JS row dict — that's what
`onSelectionChange` publishes to `cellSelections` and what
`EventDetailPanel` substitutes into its template. Move this from
"every row, eagerly" to "one row, on click", and drop the
`MapEvent` wrapper — selection identity is the row index, and the
panel only ever reads the formatted column dict:

```ts
// In overlay.ts
export type Row = Record<string, string>

export function materializeRow(table: Table, rowIndex: number): Row {
  const row: Row = {}
  // Field iteration preserves SELECT order, so the editor's
  // "Available columns" affordance and any future per-row UI keep
  // their existing ordering guarantees.
  for (const field of table.schema.fields) {
    const col = table.getChild(field.name)
    if (!col) continue
    const value = col.get(rowIndex)
    if (value === null || value === undefined) continue
    row[field.name] = formatArrowValue(value, field.type)
  }
  return row
}
```

`formatArrowValue` (`notebook-utils.ts:277`) is already the right
formatter — exported by the #1053 plan and reused here verbatim.

The `MapEvent` interface is deleted. Its fields were redundant or
unused downstream:
- `id` — only used by the current `InstancedMarkers` to translate
  `selectedId` back to an array index via `findIndex`. With
  row-index identity, the lookup goes away.
- `x`/`y`/`z` — cached in `overlay.positions`, never read off
  `MapEvent` by the panel.
- `time` — never read off `MapEvent` by the panel either; the panel
  substitutes `$time` from `row.time` (a formatted string).
- `row` — kept, now the whole shape.

`EventDetailPanel` takes `row: Row` directly (see Phase 3).

### Selection model

Move `selectedEvent: MapEvent | null` → `selectedRowIndex: number | null`
in `MapCell`. Materialization happens at the edge — once for the
panel, once for `onSelectionChange`. The cell narrows
`OverlayResult` once near the top, then passes the success-branch
`Overlay` down to everything that reads `.table` / `.positions`:

```ts
const overlayResult = useMemo(
  () => (sourceTable ? buildOverlay(sourceTable) : null),
  [sourceTable],
)
// Narrow once; `overlay` is `Overlay | null` from here on.
const overlay = overlayResult?.ok ? overlayResult.overlay : null

const [selectedRowIndex, setSelectedRowIndex] = useState<number | null>(null)

// Clear selection synchronously when the source table changes, before
// `selectedRow` is derived for the new overlay. A post-commit useEffect
// would leave one render where the stale row index materializes against
// the new table, surfacing a wrong row in the panel for that frame.
// Mirrors the `clearedForUrl` pattern in MapViewer.tsx:723-731, which
// likewise only mutates local state during render.
const [overlayForSelection, setOverlayForSelection] = useState(overlay)
if (overlayForSelection !== overlay) {
  setOverlayForSelection(overlay)
  setSelectedRowIndex(null)
}

// Publish the clear to upstream cells *after commit*. onSelectionChange
// resolves to updateCellSelection → executeFromCell → setCellStates on
// the parent (useCellExecution.ts:316); calling it during render would
// trigger React's "Cannot update a component while rendering a different
// component" warning. updateCellSelection no-ops when no prior selection
// existed (useCellExecution.ts:388), so the initial-mount fire is safe.
useEffect(() => {
  onSelectionChangeRef.current?.(null)
}, [overlay])

const handleSelectByRowIndex = useCallback((rowIndex: number | null) => {
  setSelectedRowIndex(rowIndex)
  if (rowIndex === null || !overlay) {
    onSelectionChangeRef.current?.(null)
  } else {
    onSelectionChangeRef.current?.(materializeRow(overlay.table, rowIndex))
  }
}, [overlay])

const selectedRow = useMemo(
  () => (selectedRowIndex !== null && overlay ? materializeRow(overlay.table, selectedRowIndex) : null),
  [selectedRowIndex, overlay],
)
```

The `selectedRow` derivation runs only on selection or table change,
so the per-row column walk only happens for one row at a time. The
detail panel takes `row: Row` directly — see Phase 3.

The render-phase clear (the `if (overlayForSelection !== overlay)`
block) replaces the *index-clearing* half of the current post-commit
`useEffect` (`MapCell.tsx:183-186`). Today's `selectedEvent` is a
fully materialized snapshot held in state, so it survives one frame
past a data swap with self-consistent stale data. The new design
*derives* `selectedRow` from `(overlay, selectedRowIndex)`, so a
stale index would materialize a wrong row from the new table on the
next render. The render-phase form keeps the invariant:
`selectedRowIndex` is always meaningful against the current
`overlay`. The *upstream-notification* half (publishing `null` to
`onSelectionChange`) stays in a post-commit `useEffect` because it
triggers parent setState through `updateCellSelection`.

### `MapViewer` / `InstancedMarkers` props

Replace the `events: MapEvent[]` prop with `overlay: Overlay` (the
success-branch payload, never the `OverlayResult` union — the cell
renders the error branch inline before reaching `MapViewer`) and a
`selectedRowIndex: number | null`. `InstancedMarkers` reads
`positions` directly:

```ts
const { table, positions } = overlay
for (let i = 0; i < table.numRows; i++) {
  const px = positions[i * 3]
  const py = positions[i * 3 + 1]
  const pz = positions[i * 3 + 2]
  // ... matrix + color, same as today
}
```

No `parseFloat`, no per-row property maps, no JS objects in the hot
loop. Marker index = row index throughout, so the raycast hit
reports the selectable identity directly:

```ts
const handleClick = useCallback(
  (e: ThreeEvent<MouseEvent>) => {
    e.stopPropagation()
    const rowIdx = e.instanceId
    if (rowIdx === undefined || rowIdx < 0 || rowIdx >= overlay.table.numRows) return
    onSelect(rowIdx === selectedRowIndex ? null : rowIdx)
  },
  [overlay, selectedRowIndex, onSelect],
)
```

Hover follows the same shape: `setHoveredIndex(e.instanceId)` stores a
row index, no inverse lookup, no `useMemo`-cached map.

### Hot-path effect dependencies

`InstancedMarkers` today re-walks `events` on every prop change
(`MapViewer.tsx:146-180`). With the new design:

- **Layout pass** (positions, scales, and per-instance *normal*
  colors for *all* markers) depends on `overlay`, `markerSize`, and
  `markerColor`. Move into its own `useEffect`. The `markerColor`
  dep is load-bearing: the normal per-instance color is derived from
  it (`new THREE.Color(markerColor)`), so changing the color knob
  must re-walk every instance to repaint.
- **Selection / hover diff** (touch only the two affected matrices
  and colors) depends on `selectedRowIndex`, `hoveredRowIndex`,
  *and* the layout-pass deps (`overlay`, `markerColor`,
  `markerSize`). The layout deps are load-bearing on this effect too:
  the layout pass writes the *normal* color over every slot,
  including the currently-selected one — if the highlight effect
  didn't also re-run, dragging the color picker mid-selection would
  leave the selected marker painted in the new normal color until
  the user re-clicked it. Declaring the layout effect first
  guarantees the highlight runs after it in render order, so the
  override wins. The previous-index state needed for the "restore
  prior highlight" step lives in a `useRef` updated at the end of
  the effect. Restore the previous marker's normal color + scale,
  then set the new one's highlight values.
  `mesh.instanceMatrix.needsUpdate = true` and `attr.needsUpdate =
  true` still fire. The hot path — selection/hover changes with the
  color/size/data knobs stable — touches O(1) instances; only when
  the layout deps actually change does the diff effect rewrite the
  two highlight slots (still O(1)) on top of the layout pass's full
  rewrite (O(N), unavoidable for a color/size change).

This is the spread-overflow / GC fix the issue calls out: a 50k-point
re-render no longer walks 50k JS objects to swap one highlight color.

### Default empty / error rendering

`MapCell` currently shows "No spatial data available. Query must
return columns: x, y, z" when `events.length === 0` (and status is
success). After the refactor:

- `overlayResult && !overlayResult.ok` → render
  `overlayResult.error` (e.g. "Missing required columns: x, z.
  Available: …", same as FlameGraph). Covers schema mismatch and
  non-finite-coordinate errors,
- `!overlay || overlay.table.numRows === 0` → render the existing
  "no spatial data" message. The `!overlay` arm covers
  `sourceTable === undefined` and is also the type narrow for the
  null-`overlayResult` case (today's `events.length === 0` quietly
  handles this because `[]` is always-an-array; the new derivation
  returns `null` when no table is present).

This makes "you forgot the columns" distinguishable from "the query
returned zero rows," which is friendlier than the current single
message.

### Effective marker size

`MapViewer.tsx:701-708` computes a marker size based on
`mapBounds.getSize()`. That uses the *GLB* bounds, not the data
bounds — unrelated to this refactor and unchanged.

## Comparison with FlameGraphCell

The issue cites `FlameGraphCell` as the pattern to follow. Lining up
the two designs side-by-side, both for what carries over and what
diverges:

### What carries over verbatim

| Aspect | FlameGraphCell | Proposed MapCell |
| --- | --- | --- |
| Index type | `FlameIndex { table, lanes, timeRange, idToName, xAxisMode, error? }` (`FlameGraphCell.tsx:93-102`) | `Overlay { table, positions }` + `OverlayResult = { ok: true; overlay } \| { ok: false; error }` |
| Index builder | `buildFlameIndex(table)` validates required cols, returns `error` in-band rather than throwing (`FlameGraphCell.tsx:107-120`) | `buildOverlay(table)` validates `x`/`y`/`z` exist *and* are numeric, returns a `Result`-style union — see "Where the map design diverges" item #7 |
| Build trigger | `useMemo` keyed on the input `table` (`FlameGraphCell.tsx:1078-1081`) | `useMemo` keyed on `sourceTable` |
| Column access | `table.getChild('begin').get(i)` everywhere (`FlameGraphCell.tsx:122-126`, `:386-390`, `:783-797`) | `table.getChild('x').get(i)` in `buildOverlay`; `table.getChild(field.name).get(rowIndex)` in `materializeRow` |
| Typed-array storage | `rowIndices: Int32Array`, `visualDepths: Int32Array` per lane (`FlameGraphCell.tsx:90-91`) | `positions: Float32Array` (length `numRows * 3`) |
| Hit testing identity | `HitResult.rowIndex` (`FlameGraphCell.tsx:254-257`); tooltip reads columns by that index | `instanceId` *is* the row index; panel materializes by that index |
| Tooltip / panel materialization | `nameCol.get(hit.rowIndex)`, `targetCol.get(hit.rowIndex)`, etc., formatted on demand inside the mouse-move handler (`FlameGraphCell.tsx:783-840`) | `materializeRow(table, rowIndex)` on click |
| Empty-state branching | distinct paths for `index.error` (schema bad) vs `index.table.numRows === 0` (no data) (`FlameGraphCell.tsx:1109-1124`) | same intent: `overlayResult.ok === false` vs `overlay.table.numRows === 0` |
| Re-execution clear | flame just rebuilds via memo on new `table` | map *also* clears selection on `sourceTable` change (existing behavior, `MapCell.tsx:183-186`) so stale row indices don't outlive their table |

### Where the map design diverges (and why)

1. **Positions are pre-baked into a `Float32Array`. Flame does not
   cache span begin/end.** Reason: in the flame graph, `begin`/`end`
   are world-space values that get *projected through view state*
   (`viewMinTime`, `pxPerMs`) on every render frame. Caching the raw
   values wouldn't help — the per-frame loop still has to do the
   projection. In the map, world-space positions go straight into
   `InstancedMesh` matrices once per data change and the GPU does the
   rest. A flat `Float32Array` lets the matrix-write loop run with
   zero JS-object intermediates and is the same buffer a future
   heatmap layer (point density, min/max normalization) would walk.
   The cost is 12 bytes × `numRows` (~1.2 MB at 100k rows), bounded
   and one-time.

2. **No `idToName`-style precomputed lookup.** Flame builds a
   `Map<bigint, string>` of id→name for O(1) parent-name resolution in
   tooltips (`FlameGraphCell.tsx:213-221`). The map cell's detail
   panel substitutes columns from the *selected* row only; nothing
   resolves cross-row references. If a future feature ever needs
   "show parent marker" or similar, the precomputed map would be
   added then, not now.

3. **Split layout effect vs single render path.** Flame has a single
   `render` callback (`FlameGraphCell.tsx:377-574`) called via
   `requestAnimationFrame` on every view change; it always rewalks
   every visible span. That's correct for flame because every camera
   change touches every visible span's screen position. In the map,
   selection/hover changes touch O(1) instances — splitting layout
   from highlight makes the difference observable on 100k markers.
   Flame's single-effect approach would walk all 100k matrices every
   time the user moves the mouse over a marker.

4. **InstancedMesh capacity stays fixed.** Flame's render path can
   grow the mesh capacity mid-frame (`FlameGraphCell.tsx:399-410`)
   because the visible-instance count is view-dependent. In the
   map, capacity = `table.numRows` is known at index build time and
   doesn't change until the next query re-execution (which rebuilds
   the index, which re-renders `<InstancedMarkers>` with new `args`).
   The flame-style "grow on demand" is dead weight here.

5. **Field reads happen inside `buildOverlay` once, not per
   render.** Flame deliberately *re-reads* `beginCol.get(row)` and
   `endCol.get(row)` inside its render loop (`FlameGraphCell.tsx:421-426`)
   because the projection depends on view state. The map's layout
   pass writes positions once on `overlay` change and never again — so
   the inline-`get` pattern would be strictly slower without
   compensating benefit. (Hover/selection diffs read the cached
   positions out of the `Float32Array`, not from Arrow.)

6. **No `xAxisMode` analog.** Flame's `xAxisMode: 'time' | 'bits'`
   exists because the same renderer serves two semantic axes. The
   map's coordinate axes are intrinsically numeric and never carry
   timestamps, so the polymorphism is unnecessary.

7. **`OverlayResult` is a discriminated union, not an index with an
   optional `error`.** Flame's `FlameIndex` carries
   `error?: string` alongside half-built data
   (`FlameGraphCell.tsx:93-102`). `buildOverlay` instead returns
   `{ ok: true; overlay } | { ok: false; error }` so the success
   and failure shapes are mutually exclusive at the type level. The
   reason: `positions` is allocated *during* the row walk, and a
   non-finite-coordinate failure mid-walk would leave a partially-
   populated buffer in the optional-error form — a typo away from
   the renderer reading garbage out the tail of the array. The
   union form lets the type system refuse any code that reads
   `positions` from a failed build. Precedent in this codebase:
   `extractChartData` and `extractMultiSeriesChartData` in
   `arrow-utils.ts` both use the same
   `{ ok: true; ... } | { ok: false; error }` shape.

### What stays in lockstep on purpose

- The `useMemo`-keyed-on-table build pattern.
- Returning the struct value (not throwing) on schema mismatch so the
  cell can render a column-specific message inline.
- Reading the table by-name (`getChild(name).get(i)`) anywhere a row
  needs to be projected back out — the same `formatArrowValue` helper
  used by `substituteMacros` is reused for column→string formatting
  so a `time` column in a Map detail template stringifies the same
  way it does in a Flame tooltip (both go through
  `notebook-utils.ts:277`).
- Identity is the Arrow row index. Both cells route clicks /
  selections by row index and materialize columns at the boundary.

## Implementation Steps

### Phase 1 — overlay data + materialization

1. **`analytics-web-app/src/components/map/overlay.ts`** (new file)
   - `export type Row = Record<string, string>`.
   - `export interface Overlay` (success-payload shape above).
   - `export type OverlayResult = { ok: true; overlay: Overlay } |
     { ok: false; error: string }`.
   - `export function buildOverlay(table: Table): OverlayResult`:
     - validate that `x`/`y`/`z` exist and are numeric (use
       `isNumericType` from `arrow-utils.ts`); if not, return
       `{ ok: false, error: 'Missing required columns: ...' }` (or
       a type-mismatch message),
     - allocate `positions = new Float32Array(numRows * 3)`,
     - walk rows once, writing `Number(col.get(i) ?? NaN)` into the
       three slots; if any slot is non-finite (`!Number.isFinite`),
       return `{ ok: false, error: 'Row ${i}: ...' }` (the partial
       `positions` is dropped on the failed-return path and never
       reaches a caller),
     - return `{ ok: true, overlay: { table, positions } }`.
   - `export function materializeRow(table: Table, rowIndex: number):
     Row` — single-row column dump using `formatArrowValue`.

   The file lives next to `MapViewer.tsx` so the boundary is colocated
   with its consumers; no need to add another `lib/` subtree.

### Phase 2 — viewer / markers

2. **`analytics-web-app/src/components/map/MapViewer.tsx`**
   - Change `MapViewerProps`: `events: MapEvent[]` →
     `overlay: Overlay` (the success-branch payload from
     `OverlayResult`, never the union), `selectedEventId?: string` →
     `selectedRowIndex: number | null`,
     `onSelectEvent: (event: MapEvent | null) => void` →
     `onSelect: (rowIndex: number | null) => void`.
   - Change `InstancedMarkersProps` the same way.
   - In `InstancedMarkers`:
     - Split the single `useEffect` (`MapViewer.tsx:146-180`) into
       a layout effect (depends on `overlay`, `markerSize`,
       `markerColor`) and a highlight effect (depends on
       `selectedRowIndex`, `hoveredRowIndex`). The layout effect
       reads `overlay.positions` directly with no `parseFloat`/JS-object
       intermediate. The highlight effect restores the previously
       highlighted marker's normal color + scale, then writes the
       new one — O(1) per selection change instead of O(N).
     - Click/pointer-over use `e.instanceId` as the row index
       directly — no translation.
     - `args={[geometry, material, overlay.table.numRows]}` so the
       instance count matches the table. `buildOverlay` has already
       errored out any row with non-finite x/y/z, so every written
       matrix is finite — `mesh.computeBoundingSphere()` is
       well-defined and raycasting works as today.
   - Delete the `MapEvent` interface (lines 6-17). `EventDetailPanel`
     and `MapCell` consume `Row` from `@/components/map/overlay`.

### Phase 3 — cell

3. **`analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`**
   - Delete `arrowTableToMapEvents` (and its export).
   - Import `buildOverlay`, `materializeRow`, and the `Row` type
     from `@/components/map/overlay`.
   - Replace the `events = useMemo(...arrowTableToMapEvents...)`
     block with two derivations:
     `overlayResult = useMemo(() => sourceTable ? buildOverlay(sourceTable) : null, [sourceTable])`
     and `overlay = overlayResult?.ok ? overlayResult.overlay : null`
     so the rest of the component reads a single `Overlay | null`.
   - Replace selection state with `selectedRowIndex: number | null`
     and derive `selectedRow` from
     `materializeRow(overlay.table, selectedRowIndex)` via `useMemo`.
   - Replace `handleSelectEvent` with `handleSelectByRowIndex`
     (publishes `materializeRow(overlay.table, rowIndex)` to
     `onSelectionChange`).
   - Change the empty-state branches:
     - `overlayResult && !overlayResult.ok` → render
       `overlayResult.error` (covers both the missing/non-numeric
       column case and the non-finite-coordinate case),
     - `!overlay || overlay.table.numRows === 0` → render "No
       spatial data available". The `!overlay` arm handles
       `sourceTable === undefined` and is also the type narrow for
       the null-`overlayResult` case (without it, `.table`
       dereferences null, and the MapViewer prop pass below has no
       `Overlay`). No `status === 'success'` guard here: a
       guard would leave `overlay === null` reaching MapViewer in
       'idle'/'error'/'blocked' states, which violates the
       non-nullable prop contract. Matches FlameGraphCell's
       unconditional empty branch (`FlameGraphCell.tsx:1117-1123`).
   - Pass `overlay={overlay}`, `selectedRowIndex={selectedRowIndex}`,
     `onSelect={handleSelectByRowIndex}` to `<MapViewer>` (only
     reachable after the two empty-state branches above, so
     `overlay` is the narrowed `Overlay` here).
   - Replace the post-commit "clear on data change" `useEffect`
     (current `MapCell.tsx:183-186`) with the split form from the
     "Selection model" snippet: a render-phase block clears
     `selectedRowIndex` synchronously (keeping it consistent with the
     current `overlay` in the same render), and a post-commit
     `useEffect` keyed on `overlay` publishes `null` to
     `onSelectionChange` — the latter must stay post-commit because
     it triggers parent setState through `updateCellSelection`.
   - `EventDetailPanel` now takes `row={selectedRow}` instead of
     `event={selectedEvent}` (see Phase 3 step 4 below).

4. **`analytics-web-app/src/components/map/EventDetailPanel.tsx`**
   - Change `EventDetailPanelProps.event: MapEvent` →
     `row: Row` (imported from `./overlay`).
   - Substitution body becomes `{ ...variables, ...row }` instead of
     `{ ...variables, ...event.row }`.
   - `useMemo` dep `event` → `row`.

### Phase 4 — tests

5. **`analytics-web-app/src/lib/screen-renderers/cells/__tests__/MapCell.test.tsx`**
   - The six `arrowTableToMapEvents` tests are reframed against the
     new boundary. All `buildOverlay` assertions are against the
     `OverlayResult` union — success cases assert
     `result.ok === true` then read `result.overlay.*`, failure
     cases assert `result.ok === false` and inspect `result.error`:
     - `buildOverlay` returns `{ ok: true, overlay }` with
       `overlay.positions` of length `numRows * 3` (row-order
       layout) and values matching the input columns.
     - `buildOverlay` returns `{ ok: false, error }` naming the row
       when any coordinate is non-finite (e.g. a row with
       `x = NaN`). New behavior — replaces the previous "skips NaN
       rows" test (`MapCell.test.tsx:75-84`), which asserted the
       silent-drop behavior being removed in this PR.
     - `buildOverlay` returns `{ ok: false, error }` when x/y/z are
       missing.
     - `buildOverlay` returns `{ ok: false, error }` when x/y/z
       exist but aren't numeric (e.g. a string-typed `x` column).
       New test — pins the schema-level check that complements
       per-row finite-value checking.
     - `materializeRow` formats every non-null column as a string,
       omits null-valued columns, formats timestamps as RFC3339.
   - The `mapMetadata` tests are unchanged.

6. **`analytics-web-app/src/components/map/__tests__/overlay.test.ts`**
   (optional — only if Phase 4 ends up testing both `buildOverlay`
   and `materializeRow` from MapCell.test.tsx, splitting them keeps
   tests close to their unit. If keeping everything in
   MapCell.test.tsx is fine, skip this file.)

7. **`__tests__/EventDetailPanel.test.tsx`** — updated for the
   `event` → `row` prop rename. The `buildEvent` helper becomes
   `buildRow` returning `Record<string, string>`; each
   `<EventDetailPanel event={...}>` becomes
   `<EventDetailPanel row={...}>`. The substitution assertions
   themselves (the `$x`/`$time`/`$from`/`$to` / link-routing /
   collision-precedence checks) are bit-identical — the panel still
   substitutes the row dict over notebook variables.

### Phase 5 — performance smoke

8. **Manual smoke (not a CI test)** — verify the issue's "50k+
   points" acceptance criterion. Use
   `python3 local_test_env/ai_scripts/start_services.py`, open a
   map notebook in the analytics web app, run a query that returns
   ~50k rows (e.g. against a synthetic dataset or a generated SQL
   `SELECT ... FROM generate_series`). Confirm:
   - the React profiler shows the `MapCell` commit time dropping
     materially (the current path is dominated by
     `arrowTableToMapEvents`),
   - selecting / hovering a marker no longer re-walks every event
     (Performance tab: the highlight effect should account for ~zero
     time, vs the current `useEffect` that re-runs the full layout
     loop on every selection change),
   - the spread-overflow path (`Math.min(...events.map(e => e.x))`,
     referenced by the issue's "spread-overflow risk") is gone — it
     never made it into the current code, and the new design has no
     spread over a marker-length array anywhere.

## Files to Modify

Frontend code:
- `analytics-web-app/src/components/map/overlay.ts` (new — `Overlay`,
  `OverlayResult`, `Row`, `buildOverlay`, `materializeRow`)
- `analytics-web-app/src/components/map/MapViewer.tsx`
- `analytics-web-app/src/components/map/EventDetailPanel.tsx` (prop rename `event` → `row`)
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`

Frontend tests:
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/MapCell.test.tsx`
- `analytics-web-app/src/components/map/__tests__/EventDetailPanel.test.tsx`
  (prop rename: `event={buildEvent(...)}` → `row={buildRow(...)}`)

No changes to:
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`
  (`formatArrowValue` is reused as-is).
- `analytics-web-app/src/components/AvailableVariablesPanel.tsx`.

## Trade-offs

**Row-index identity vs string `id`.** The current code derives a
string id (`${row['process_id'] ?? 'unknown'}-${i}`) and selection
compares by id. Switching to a row-index identity inside the cell
eliminates a per-row string allocation and a `findIndex` scan on
every selection change. The derived `id` was never consumed
externally (no caller reads `MapEvent.id` outside `MapViewer`'s own
selection plumbing), so dropping it has no observable effect — the
panel substitutes `$process_id` from `row.process_id` directly, and
`cellSelections` already publishes the row dict, not the wrapper.

**Two effects in `InstancedMarkers` vs one.** Splitting the layout
pass from the highlight pass lets selection/hover updates touch O(1)
instances. The existing single-effect form is simpler to read but
re-walks every marker on every selection change — exactly the
"render cost should be GPU-bound" criterion in the acceptance list.
Two effects is the right shape for the new scale target.

**Detect non-finite x/y/z and error, vs silent-drop or pass-through.**
The current `arrowTableToMapEvents` silently skips rows whose
`parseFloat(x|y|z)` is NaN — a filter preserved by a test, not by any
consumer. Passing those values through to `InstancedMesh.setMatrixAt`
is not an option: `mesh.computeBoundingSphere()` unions every
instance sphere without filtering, and `Sphere.union` doesn't skip a
NaN-centered sphere. One NaN row leaves `boundingSphere.center` NaN,
`Ray.intersectsSphere` returns false, and the raycast bails before
the per-instance loop runs — every marker becomes unclickable. The
chosen path is to validate at build time: `buildOverlay` returns
`{ ok: false, error }` naming the first offending row, and the cell
renders that error message inline. This is stricter than the current
silent-drop behavior (a single bad row breaks the build instead of
disappearing), but the failure mode is loud and actionable rather
than mysterious.

**Materialize on click vs cache the last selection's row.** Both work;
clicking a marker is rare relative to render frames, so the cache
doesn't pay for itself. `materializeRow` runs O(C) per click which
is negligible.

**Collapse `MapEvent` into `Row` vs preserve the wrapper.** The
current `MapEvent` carries `id`/`time`/`x`/`y`/`z`/`row`, but with
row-index identity none of the typed fields are read downstream:
`id` was only used to map `selectedId` → array index via `findIndex`,
and the panel substitutes `$time`/`$x`/`$y`/`$z` from the formatted
`row` dict. Removing the wrapper drops a per-selection JS object and
makes the panel's input contract exactly what it consumes: one row
of the table.

**No `transformEvents` / `HeatmapLayer` plumbing in this PR.** Neither
exists today; the issue lists them as future consumers. The
`Overlay` boundary (positions as `Float32Array`, direct table access
for column queries) is what those components will need when they
come back. No speculative interface is added for them — extra
buffers (bounds, color, size, alpha) grow `Overlay` when their first
consumer lands, not before.

**Name `Overlay` instead of `MapIndex` / `MapPoints`.** Issue
[#1055](https://github.com/madesroches/micromegas/issues/1055) extends
the cell to render arbitrary primitives (boxes, future cylinders /
polylines / polygons) with column-bound visual channels (size, color,
alpha, yaw). Naming this struct after a primitive (`Points`) or as an
index (`MapIndex`, the FlameGraph-borrowed term — see "Comparison")
would mislead once those channels arrive. `Overlay` describes the
struct's role — data prepared for rendering as an overlay on the
GLB-loaded map — and survives the channel expansion: #1055 grows the
struct with per-channel buffers rather than replacing it.

**`OverlayResult` is a discriminated union, not an optional `error`
on the data type.** Matches the chart helpers in `arrow-utils.ts`
(`extractChartData`, `extractMultiSeriesChartData`) rather than
`buildFlameIndex`'s optional-`error` shape. Detailed reasoning in "Where the map design
diverges" item #7: `positions` is allocated during the row walk, so
a failure mid-walk leaves a partial buffer; the union form makes the
success and failure shapes mutually exclusive at the type level so
no caller can read `positions` from a failed build. Returning the
union value (vs throwing) still lets the cell render a column-
specific error inline without bubbling to the `ErrorBoundary`.

## Documentation

- No public-facing docs change. The "Map" section of
  `mkdocs/docs/web-app/notebooks/cell-types.md` continues to document
  the SQL contract (`x`, `y`, `z` required), which is unchanged.

## Testing Strategy

- **Unit** (`MapCell.test.tsx`): reframe the six existing
  `arrowTableToMapEvents` tests onto `buildOverlay` +
  `materializeRow`, asserting against the `OverlayResult` union.
  Add coverage for the new error paths (missing x/y/z, non-numeric
  x/y/z, non-finite coordinate value).
- **Component** (`EventDetailPanel.test.tsx`): updated for the
  `event` → `row` prop rename; the substitution assertions
  themselves (`$col`, link routing, macro precedence) are
  bit-identical because the panel's substitution body still reads
  from the same row dict.
- **Integration** (manual smoke, Phase 5): verify 50k-point render
  with React Profiler; verify selection/hover do not re-walk the
  full marker set; verify the panel still renders the correct row
  on click.

## Open Questions

None blocking implementation.
