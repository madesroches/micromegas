# Flame Graph Cell Type Plan

## Overview

Add a new `flamegraph` notebook cell type that renders Perfetto-style flame graph visualizations of CPU traces. Spans are displayed as stacked horizontal bars arranged by depth and time, grouped into lanes (threads or async tracks). The cell accepts SQL query results, supporting both remote server queries and local WASM queries.

**GitHub Issue**: #917

## Status: IMPLEMENTED

All phases complete. The flame graph cell renders thread and async spans with interactive pan/zoom, tooltips, and drag-to-zoom selection.

Key implementation decisions that diverged from the original plan:
- **Async span layout uses DFS tree-walk** instead of simple depth-based stacking. Parent-child tree is built from `id`/`parent` columns, then laid out with DFS order + row-end tracking so children appear directly below their parent and non-overlapping sequential siblings share visual rows.
- **`id` and `parent` columns are required** for async spans (used for tree-based layout and tooltip parent name resolution).
- **WASD controls** for pan/zoom instead of wheel zoom (avoids conflict with browser zoom). W/S zoom cursor-anchored, A/D pan horizontally, wheel scrolls vertically.
- **`overflow-hidden`** on container to prevent tooltip from triggering scrollbars.

## Current State

Fully functional flame graph cell with:
- Three.js WebGL rendering via InstancedMesh for span rectangles
- Canvas2D overlay for text labels and time axis
- DOM tooltip with span name, duration, depth, parent info
- DFS tree layout for async spans with row reuse for non-overlapping chains
- Depth-based layout for thread spans
- WASD pan/zoom, drag-to-zoom, double-click reset
- `process_spans(process_id, types)` table function for unified span queries

## Data Schema

The cell expects query results with these columns:

| Column | Type | Required | Description |
|--------|------|----------|-------------|
| `id` | integer | yes | Span ID (used for tree layout and tooltip) |
| `parent` | integer | yes | Parent span ID (used for tree layout and tooltip) |
| `name` | string | yes | Span/function name |
| `begin` | timestamp | yes | Span start time |
| `end` | timestamp | yes | Span end time |
| `depth` | integer | yes | Nesting depth (0 = top-level) |
| `lane` | string | no | Grouping identifier (e.g., thread name). All spans in one lane if absent |

Optional enrichment columns (displayed in hover tooltip):
- `target` — module/target name
- `filename` — source file
- `line` — source line number

## Design

### Rendering Architecture

Three-layer rendering following the Perfetto/Chrome DevTools pattern:

```
FlameGraphCell (renderer)
├── Three.js WebGL canvas (bottom) — span rectangles via InstancedMesh
├── Canvas2D overlay (middle) — span name labels, time axis ticks
└── DOM layer (top) — hover tooltip, lane name labels
```

**Why Three.js**: Three.js is coming to the project for 3D heatmap rendering. Using it here shares the dependency cost (~600KB paid once). `InstancedMesh` with `OrthographicCamera` gives Perfetto-style single-draw-call rendering for tens of thousands of rectangles with smooth pan/zoom built into the camera system.

**Why Canvas2D overlay for text**: Three.js text rendering is its weakest area (TextGeometry crashes at ~1000 labels, canvas textures blur on zoom, CSS2DRenderer degrades at 300+ labels). Perfetto, Chrome DevTools, and speedscope all use Canvas2D overlaid on WebGL for text. `ctx.fillText()` handles thousands of labels per frame with native font rendering at any zoom level.

**Why plain Three.js + useRef over R3F**: The flame graph is a single `InstancedMesh` — no component tree needed. Plain Three.js with a React ref is simpler for this isolated 2D use case. R3F will be used separately for the future 3D heatmap cell where component composition matters. Both coexist fine in the same app (separate WebGL contexts).

### Data Model — Zero-Copy Arrow Access

Keep data in the Arrow table and build a lightweight index over it. This avoids allocating thousands of objects and leverages Arrow's compact columnar storage — which also mirrors the columnar buffer layout that Perfetto uses for GPU uploads.

```
Arrow Table → buildFlameIndex(table) → FlameIndex
```

```typescript
interface LaneIndex {
  id: string
  name: string
  maxDepth: number
  /** Row indices into the Arrow table belonging to this lane, sorted by begin time */
  rowIndices: Int32Array
}

interface FlameIndex {
  table: Table              // original Arrow table — columns accessed by index during render
  lanes: LaneIndex[]
  timeRange: { min: number; max: number }
  error?: string
}
```

The Arrow column vectors (`table.getChild('begin')`, etc.) are accessed directly by row index during instance matrix updates, Canvas2D label rendering, and hit-testing. `timestampToMs()` is called per-access (cheap arithmetic).

Index construction:
1. Validate required columns (`id`, `parent`, `name`, `begin`, `end`, `depth`)
2. Single pass over rows: bucket row indices by `lane` value (or single default lane if column absent), track min/max timestamps, track max depth per lane
3. Sort each lane's `rowIndices` by begin timestamp (for efficient culling and binary-search hit-testing)
4. For async lanes: build parent-child tree, compute visual depths via DFS + row-end tracking (see `computeAsyncVisualDepths`)
5. For thread lanes: use depth column directly as visual depth

### Three.js WebGL Layer (Rectangles)

Setup (in `useEffect` on canvas ref):
- `WebGLRenderer` attached to canvas element
- `OrthographicCamera` — left/right/top/bottom set to pixel dimensions, zoom controlled by wheel
- `Scene` with a single `InstancedMesh` using `PlaneGeometry(1, 1)` and a basic `MeshBasicMaterial` with `vertexColors: true`

Per-span instance data:
- **Matrix**: `setMatrixAt(i, matrix)` — encodes position (x, y) and scale (width, height) for each span rectangle
- **Color**: `setColorAt(i, color)` — deterministic hash of span name

Update cycle (on data change or pan/zoom):
1. Compute visible time range from camera
2. For each lane, binary search `rowIndices` to find first visible span
3. Iterate forward, setting instance matrix/color for each visible span
4. Set `instancedMesh.count` to number of visible spans (culling)
5. Mark `instanceMatrix.needsUpdate = true`, `instanceColor.needsUpdate = true`
6. `renderer.render(scene, camera)`

Pan/zoom:
- **Wheel zoom**: Adjust `camera.zoom` centered on cursor position (logarithmic scaling like Perfetto)
- **Click-drag pan**: Translate `camera.position` based on drag delta
- **Drag-to-zoom**: Selection overlay (rendered in Canvas2D layer), then snap camera to selected time range
- All via standard DOM event listeners on the canvas, updating camera and requesting re-render

### Canvas2D Overlay Layer (Text Labels)

A second `<canvas>` element positioned on top of the WebGL canvas via CSS (`position: absolute; pointer-events: none`). Redrawn on each frame after the WebGL render:

1. Clear canvas
2. For each visible span where rectangle width > ~40px: draw `ctx.fillText(name)` clipped to rectangle bounds
3. Draw time axis ticks at bottom
4. Use `ctx.save()`/`ctx.rect()`/`ctx.clip()` to clip text to span width (or measure + truncate with ellipsis)

### DOM Layer (Tooltip + Lane Names)

- **Lane names**: `<div>` elements with `position: absolute`, fixed to left edge. Updated when vertical scroll changes.
- **Hover tooltip**: Single `<div>` positioned at mouse coordinates. Shows span name, duration, target, file:line. Read directly from Arrow columns by row index from hit-test result.

### Hit-Testing

CPU-side coordinate math (same approach as Perfetto — no GPU picking needed):
1. Convert mouse (x, y) to data coordinates using camera inverse projection
2. Determine which lane the y-coordinate falls in, then which depth band
3. Binary search that lane's sorted `rowIndices` on begin time to find candidate spans
4. Check if mouse x falls within [begin, end] of any candidate at the matching depth
5. Return the row index (or null) — tooltip reads fields directly from Arrow columns

### Color Scheme

Deterministic name → color mapping using a string hash into the brand tricolor palette. All three brand color families are used (~60% warm, ~40% cool), giving maximum span differentiation while staying on-brand. See `tasks/flame/flamegraph_mockup_B_tricolor.html` for the visual reference.

```typescript
// Brand-derived tricolor palette: Rust family, Blue family, Gold family
const FLAME_PALETTE = [
  // Rust family (brand Rust #bf360c → #8d3a14)
  '#8d3a14', '#a33c10', '#bf360c', '#c94e1a', '#d46628',
  // Blue family (brand Blue #1565c0 → #0d47a1)
  '#0d47a1', '#1565c0', '#1976d2', '#1e88e5', '#2196f3',
  // Gold family (brand Gold #ffb300 → #e6a000)
  '#e6a000', '#ecae1a', '#ffb300', '#ffc107', '#ffd54f',
]

// Contrast-aware text color: light on blue spans, dark on warm spans
const BLUE_INDICES = new Set([5, 6, 7, 8, 9])

function spanColor(name: string): [color: string, textLight: boolean] {
  let hash = 0
  for (let i = 0; i < name.length; i++) {
    hash = ((hash << 5) - hash + name.charCodeAt(i)) | 0
  }
  const idx = Math.abs(hash) % FLAME_PALETTE.length
  return [FLAME_PALETTE[idx], BLUE_INDICES.has(idx)]
}
```

### Default SQL Queries

**Thread spans** (default):
```sql
SELECT name, begin, end, depth, thread_name as lane
FROM process_spans('$process_id', 'both')
WHERE begin >= TIMESTAMP '$from'
  AND end <= TIMESTAMP '$to'
ORDER BY lane, begin
```

**Async spans** (alternate template shown in editor docs):
```sql
WITH matched AS (
  SELECT b.name, b.time as begin, e.time as end, b.depth, 'async' as lane
  FROM (SELECT * FROM view_instance('async_events', '$process_id')
        WHERE event_type = 'begin') b
  JOIN (SELECT * FROM view_instance('async_events', '$process_id')
        WHERE event_type = 'end') e
  ON b.span_id = e.span_id
)
SELECT name, begin, end, depth, lane
FROM matched
WHERE begin >= TIMESTAMP '$from' AND end <= TIMESTAMP '$to'
ORDER BY begin
```

### Cell Config Type

```typescript
// Reuses QueryCellConfig — add 'flamegraph' to its type union
interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log' | 'propertytimeline' | 'swimlane' | 'transposed' | 'flamegraph'
  sql: string
  options?: Record<string, unknown>
  dataSource?: string
}
```

No new config interface needed — `QueryCellConfig` with `options: {}` is sufficient.

## Dependencies

**New npm dependency:**
- `three` (~600KB minified) — shared with future 3D heatmap cell

No R3F or drei needed for the flame graph. The 3D heatmap cell will add those when it's implemented.

## Implementation Steps — ALL COMPLETE

### Phase 1: Type System & Registry — DONE

1. ~~Add `'flamegraph'` to `CellType` union in `notebook-types.ts`~~
2. ~~Add `'flamegraph'` to `QueryCellConfig.type` union in `notebook-types.ts`~~
3. ~~Add default SQL to `DEFAULT_SQL` in `notebook-utils.ts`~~
4. ~~Add `three` to `package.json` dependencies~~
5. ~~Create `FlameGraphCell.tsx` with renderer, editor, `execute` method, and metadata export~~
6. ~~Import and register `flamegraphMetadata` in `cell-registry.ts`~~

### Phase 2: Data Indexing — DONE

7. ~~Implement `buildFlameIndex(table: Table): FlameIndex`~~
   - Thread lanes: depth column used directly
   - Async lanes: DFS tree-walk via `computeAsyncVisualDepths()` (extracted, tested)

### Phase 3: WebGL Renderer — DONE

8. ~~Three.js setup: WebGLRenderer, OrthographicCamera, InstancedMesh, resize handling~~
9. ~~Instance update loop: visible range culling, matrix/color updates, deterministic color~~

### Phase 4: Pan/Zoom — DONE

10. ~~WASD controls: W/S cursor-anchored zoom, A/D horizontal pan~~
11. ~~Drag-to-zoom selection overlay (Canvas2D layer)~~
12. ~~`onTimeRangeSelect` callback (Alt+drag propagates to notebook)~~
13. ~~Wheel vertical scroll, double-click zoom reset~~

### Phase 5: Text & Interaction — DONE

14. ~~Canvas2D overlay for span name labels and time axis~~
15. ~~CPU-side hit-testing (lane detection + depth band + time range check)~~
16. ~~DOM tooltip: name, duration, depth, id, parent name, target, file:line~~

### Phase 6: Editor — DONE

17. ~~FlameGraphCellEditor with SQL editor, available variables panel, validation~~

### Phase 7: Async Depth Fix (added during implementation)

18. ~~Fix `SpanContextFuture` to push/pop parent span on every poll (replaces broken `SpanScope` RAII guard)~~
19. ~~Add stack depth padding in `SpanContextFuture` to preserve depth across spawn boundaries~~
20. ~~Tests: 9 async depth tracking tests including cross-spawn and cross-yield scenarios~~

## Files Modified

**Created:**
- `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx` — renderer, editor, metadata, `computeAsyncVisualDepths`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/FlameGraphLayout.test.ts` — 5 layout tests
- `rust/tracing/src/spans/instrumented_future.rs` — `SpanContextFuture` (replaced `SpanScope`)

**Modified:**
- `analytics-web-app/src/lib/screen-renderers/notebook-types.ts` — added `'flamegraph'` to type unions
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` — registered `flamegraphMetadata`
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` — added default SQL
- `analytics-web-app/package.json` — added `three` dependency
- `rust/tracing/src/lib.rs` — updated prelude exports (`SpanContextFuture` replaces `SpanScope`)
- `rust/tracing/tests/async_depth_tracking_tests.rs` — added 3 tests for spawn/depth scenarios

## Trade-offs

**Three.js WebGL + Canvas2D overlay vs pure Canvas2D**: Three.js chosen for smooth continuous pan/zoom (camera-based, hardware accelerated) and single-draw-call instanced rendering at scale. Canvas2D would require manual redraw on every pan/zoom frame. The overlay pattern (WebGL geometry + Canvas2D text) is proven by Perfetto, Chrome DevTools, and speedscope.

**Three.js vs Pixi.js**: Three.js chosen because it's coming to the project for 3D heatmaps — sharing one dependency is better than adding both. Pixi.js (220KB) would be a better standalone choice for pure 2D, but the shared dependency argument wins.

**Plain Three.js + useRef vs R3F**: Plain Three.js is simpler for this isolated single-InstancedMesh scene. R3F's component model adds no value here but will be used for the more complex 3D heatmap cell. Both coexist fine (separate canvas elements = separate WebGL contexts).

**WASM rendering**: Not needed. WASM canvas rendering via `web-sys` is slower than JS due to FFI overhead per draw call. The compute work (indexing, culling) is O(n) and fast in JS. The proven WASM flame graph approach (flame.cat, zymtrace) uses WASM compute + WebGL rendering — which is what Three.js already provides on the rendering side. Future path if needed: move layout computation to `datafusion-wasm`.

**d3.js**: d3-scale and d3-axis don't add value when Three.js handles coordinate transforms via camera. d3-zoom could be useful for pan/zoom behavior but Three.js camera manipulation achieves the same result. May reconsider d3-zoom if the custom pan/zoom implementation proves complex.

**Async span layout — DFS tree-walk vs flat depth-based**: Thread spans use the depth column directly (call-stack depth, no overlap possible). Async spans use a DFS tree-walk layout built from `id`/`parent` columns: children are placed directly below their parent, non-overlapping sequential siblings share visual rows (row-end tracking), and overlapping concurrent siblings get bumped to the next row. This was chosen over flat depth-based stacking which separated children from parents when intermediate depth levels had many concurrent spans. The layout algorithm is extracted into a tested `computeAsyncVisualDepths()` function.

## Documentation

- **Update**: `mkdocs/docs/web-app/notebooks/cell-types.md` — add Flame Graph section with configuration table, required columns, example SQL for both thread and async spans, and a screenshot
- **Update**: `mkdocs/docs/query-guide/async-performance-analysis.md` — add a section showing how to visualize async spans in a flame graph cell

## Testing Strategy

1. Create a notebook with a process variable and flame graph cell using the default thread spans SQL — verify spans render correctly with lane grouping by thread
2. Test with async spans SQL — verify begin/end event matching produces correct span bars
3. Test hover tooltip — verify name, duration, and source location display
4. Test drag-to-zoom — verify time range selection callback fires and downstream cells re-execute
5. Test smooth pan/zoom — verify wheel zoom (cursor-centered) and click-drag pan
6. Test span name labels — verify text appears for wide spans, hidden for narrow ones
7. Test with empty results — verify graceful empty state
8. Test with missing columns — verify helpful error message listing missing columns
9. Test with WASM data source — verify local query execution works
10. Test WebGL context cleanup — verify no context leaks on cell unmount
11. Run `yarn lint` and `yarn type-check` to verify no type errors

## Future Improvements

- **Level-of-Detail (LOD) system**: At low zoom levels, thousands of tiny spans create visual noise and waste rendering effort. A LOD system would merge closely-spaced spans into aggregate blocks when they're smaller than a pixel threshold. Two approaches: (a) client-side merging (like Legion's `lgn-analytics-gui` — logarithmic LOD with `Math.floor(Math.log(pixelSizeNs) / Math.log(100))` and merge thresholds), or (b) SQL-level bucketing (like Perfetto's mipmap tables — pre-aggregated data at different resolutions returned by the query). Client-side is simpler to start with; SQL-level scales better for very large datasets.

## Open Questions

1. **Span count limit**: Should we enforce a max span count (e.g., 50k) with a warning, or let the InstancedMesh handle whatever the query returns? WebGL instanced rendering handles 100k+ rects efficiently, but the Arrow → instance buffer update loop may become a bottleneck.
