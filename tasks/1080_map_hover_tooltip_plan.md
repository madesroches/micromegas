# Map Cell: Hover Tooltips for Markers Plan

Addresses [issue #1080](https://github.com/madesroches/micromegas/issues/1080).

## Overview

Today hovering a map marker only changes its color/scale; to read any row data
the user must click, which opens the docked detail panel. This plan renders the
**existing `detailTemplate`** as a small floating panel that follows the cursor
while a marker is hovered — the same Markdown content as the docked panel, shown
transiently. No new template option. Click → docked panel behavior is unchanged;
hover is a transient preview using the same content. If `detailTemplate` is blank
the hover behavior is unchanged (highlight only, no floating panel).

## Current State

### Hover lives entirely inside the marker mesh

`MapInstancedMarkers` (`analytics-web-app/src/components/map/MapInstancedMarkers.tsx`)
owns hover as local state and uses it only to drive the GPU highlight pass:

- `hoveredRowIndex` state (`MapInstancedMarkers.tsx:37`), cleared on overlay swap
  (`:44-48`).
- `handlePointerOver` (`:327-337`) sets `hoveredRowIndex` from `e.instanceId` and
  flips the body cursor to `pointer`; `handlePointerOut` (`:339-342`) clears both.
- The highlight diff pass (`:213-298`) repaints the hovered slot with
  `COLOR_HOVERED_RGBA` / `SCALE_HOVERED`.

There is **no pointer-position tracking and no `onPointerMove`** — hover is purely
enter/leave today, and nothing about the hovered row is surfaced outside the mesh.

### The docked detail panel

`EventDetailPanel` (`analytics-web-app/src/components/map/EventDetailPanel.tsx`)
renders the detail template:

- Evaluates the template via `evaluateTemplate(..., { row, columnTypes,
  bareColumnsFromRow: true })` in a `useMemo` keyed on the row + template + macro
  inputs (`:57-71`).
- Renders a `TemplateWarningBanner` + `react-markdown` (remark-gfm) with `prose`
  styling and a custom `MarkdownLink` (`:73-89`).
- Chrome: absolutely positioned `bottom-4 left-4`, `max-w-[50%] max-h-[60%]
  overflow-auto`, a close button, `z-10` (`:74-81`).

### MapCell already holds everything the template needs

`MapCell` (`analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`):

- `detailTemplate` with default fallback (`:312-313`).
- `selectedRow = rowValues(overlay.table, selectedRowIndex)` memoized
  (`:327-333`) and `columnTypes = columnTypeMap(overlay.table)` memoized
  (`:338-341`) — both from `overlay.ts` (`rowValues` `:560`, `columnTypeMap`
  `:571`).
- `variables`, `timeRange`, `cellResults`, `cellSelections` (already passed to
  `EventDetailPanel`, `:436-447`).
- Renders inside a `relative w-full h-full overflow-hidden` container
  (`:393`) — note the `overflow-hidden`, which would clip an in-container
  absolutely-positioned tooltip near the cell edges.

`MapViewer` (`MapViewer.tsx:103-249`) wraps the R3F `<Canvas>` and threads
`overlay/constants/shape/selectedRowIndex/onSelect` down to `MapInstancedMarkers`
(`:214-220`).

### Tests / docs

- `EventDetailPanel.test.tsx` renders the panel directly and asserts macro
  substitution.
- `MapCell.test.tsx` exercises helpers/config (it does not mount `MapViewer`).
- Detail-template docs: `mkdocs/docs/web-app/notebooks/cell-types.md:279-320`.

## Design

```
MapInstancedMarkers              MapViewer          MapCell
  pointermove (rAF-throttled) ──onHover(idx,x,y)──► onHover ──► hover state {rowIndex,x,y}
  pointerover/out             ──onHover(...|null)──►                │
  (existing highlight state                                        ├─ hoveredRow = rowValues(idx)  (memo on idx)
   stays internal, unchanged)                                      └─ <MapHoverTooltip> (portal, fixed, follows cursor)
                                                                          │
                       EventDetailContent  ◄── shared ──►  EventDetailPanel (docked, unchanged)
```

### 1. Share the detail-content rendering (DRY)

Extract the template-eval + warning banner + Markdown block out of
`EventDetailPanel` into a presentational `EventDetailContent` so the docked panel
and the new tooltip render identical content with different chrome.

New `analytics-web-app/src/components/map/EventDetailContent.tsx`:

```tsx
interface EventDetailContentProps {
  row: Record<string, unknown>
  columnTypes: Map<string, DataType>
  template: string
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
}
```

It holds the existing `useMemo(evaluateTemplate(...))`, the `prose` `<div>`, the
`TemplateWarningBanner`, and `<Markdown components={{ a: MarkdownLink }}>`
(move `MarkdownLink` here too). `EventDetailPanel` keeps only its chrome (the
`bottom-4 left-4` panel + close button) and renders `<EventDetailContent .../>`
inside it — its public props and behavior are unchanged.

### 2. Surface hover (row index + cursor position) from the mesh

`MapInstancedMarkers` keeps `hoveredRowIndex` for the highlight (open/closed: the
highlight logic is untouched). Add an additive, optional callback:

```ts
onHover?: (rowIndex: number | null, clientX: number, clientY: number) => void
```

- New `handlePointerMove` bound on the `<instancedMesh>` (`onPointerMove`),
  **rAF-throttled** (at most one update per frame — satisfies "throttle on
  pointermove"): stash the latest `e.instanceId` + `e.clientX/clientY` in a ref;
  if no frame is pending, `requestAnimationFrame` a flush that calls
  `setHoveredRowIndex(idx)` and `onHover?.(idx, x, y)`. `setHoveredRowIndex` with
  an unchanged value is a no-op for the highlight effect, so per-move moves only
  reposition the tooltip; the row stays the same.
- `handlePointerOver` also calls `onHover` (so the tooltip appears on enter even
  before the first move). `handlePointerOut` calls `onHover(null, 0, 0)` and
  cancels any pending rAF.
- Cancel the pending rAF on unmount (alongside the existing cursor-restore
  effect at `:310-314`).

`e.clientX/clientY` come from the native `PointerEvent` on the R3F event — viewport
coordinates, which the tooltip uses directly with `position: fixed`.

### 3. Thread `onHover` through MapViewer

Add `onHover?: (rowIndex, clientX, clientY) => void` to `MapViewerProps`, pass it
straight into `<MapInstancedMarkers onHover={onHover} ... />` (`MapViewer.tsx:214`).
MapViewer is a pass-through; no state added there.

### 4. MapCell: hover state + tooltip render

```ts
const [hover, setHover] = useState<{ rowIndex: number; x: number; y: number } | null>(null)

const handleHover = useCallback((rowIndex: number | null, x: number, y: number) => {
  setHover(rowIndex === null ? null : { rowIndex, x, y })
}, [])

// Memoized on rowIndex (not x/y): cursor movement repositions without
// re-deriving the row or re-evaluating the template.
const hoveredRow = useMemo(
  () => (hover && overlay ? rowValues(overlay.table, hover.rowIndex) : null),
  [hover?.rowIndex, overlay],
)
```

**Clear stale hover on overlay swap (render-phase).** The mesh only calls
`onHover` from pointer events; when `overlay` changes with no pointer event,
`MapInstancedMarkers` clears its internal `hoveredRowIndex` via its render-phase
derivation (`MapInstancedMarkers.tsx:44-48`) but never calls `onHover(null)`, so
MapCell's lifted `hover` keeps the old `rowIndex`. The `hoveredRow` memo would
then re-derive `rowValues(overlay.table, hover.rowIndex)` against the *new* table
— and `rowValues` (`overlay.ts:560-568`) does no bounds check, so the tooltip
renders stale/empty content until the next pointer event. Mirror the existing
selection guard (`overlayForSelection`, `MapCell.tsx:282-286`) by tracking the
overlay identity in state and resetting `hover` to `null` during render when it
changes:

```ts
const [hoverOverlay, setHoverOverlay] = useState(overlay)
if (hoverOverlay !== overlay) {
  setHoverOverlay(overlay)
  if (hover !== null) setHover(null)
}
```

This runs before the `hoveredRow` memo derives against the new table, so the
tooltip is gone on the swap and reappears only on the next real pointer event.

Pass `onHover={handleHover}` to `<MapViewer>`. Render the tooltip as a sibling of
the docked panel:

```tsx
{hover && hoveredRow && columnTypes && detailTemplate.trim() && (
  <MapHoverTooltip
    x={hover.x}
    y={hover.y}
    row={hoveredRow}
    columnTypes={columnTypes}
    template={detailTemplate}
    variables={variables}
    timeRange={timeRange}
    cellResults={cellResults}
    cellSelections={cellSelections}
  />
)}
```

- `detailTemplate.trim()` guard implements "blank template → highlight only, no
  panel" (the `options.detailTemplate ?? DEFAULT` fallback means a template is
  normally present; this handles an author who clears it to empty).
- The tooltip shows for any hovered marker, **including the selected one** — the
  transient cursor preview is consistent regardless of selection, even though the
  docked panel also shows that row.

### 5. New `MapHoverTooltip` component

`analytics-web-app/src/components/map/MapHoverTooltip.tsx` — cursor-following
chrome around the shared `EventDetailContent`:

- **Portal to `document.body`** via `createPortal`. The cell container is
  `overflow-hidden` (`MapCell.tsx:393`); a portal + `position: fixed` escapes
  that clip and any ancestor `transform` containing-block surprises.
- `position: fixed`, `pointer-events-none` (must never intercept pointer events,
  or it would steal the move/trigger `pointerout` and flicker), a high `z-index`
  (above the docked panel's `z-10`), `max-w`/`max-h` like the docked panel,
  reusing the same `bg-app-panel border rounded-lg shadow-lg` styling. No close
  button.
- **Auto-position to avoid edge clipping:** default offset of the cursor
  (e.g. `+14px` right / `+14px` down). A `useLayoutEffect` measures the rendered
  tooltip's `getBoundingClientRect()` against `window.innerWidth/innerHeight`;
  if it would overflow right, flip to the cursor's left; if it would overflow
  bottom, flip above; finally clamp into the viewport with a small margin. Run in
  a layout effect (pre-paint) and key the placement recompute on `x/y` + measured
  size so there is no visible jump. Content is memoized on the row, so flips only
  re-measure, they don't re-evaluate the template.

## Implementation Steps

1. **Extract shared content** — new `EventDetailContent.tsx` (move the eval
   `useMemo`, `prose` div, `TemplateWarningBanner`, `Markdown`, and `MarkdownLink`
   out of `EventDetailPanel`). Refactor `EventDetailPanel.tsx` to render
   `<EventDetailContent>` inside its existing chrome; props/behavior unchanged.
2. **Mesh hover callback** — add `onHover` to `MapInstancedMarkers`; add the
   rAF-throttled `handlePointerMove` (+ `onPointerMove` on the mesh); call
   `onHover` from over/out; cancel rAF on pointerout and unmount.
3. **Thread through MapViewer** — add `onHover` to `MapViewerProps`, pass to
   `MapInstancedMarkers`.
4. **New `MapHoverTooltip.tsx`** — portal + fixed positioning + auto-flip/clamp
   wrapping `EventDetailContent`.
5. **MapCell wiring** — `hover` state, `handleHover`, `hoveredRow` memo, the
   render-phase clear of `hover` on overlay swap (mirroring `overlayForSelection`),
   pass `onHover` to `MapViewer`, render `MapHoverTooltip` with the blank-template
   and selected-row guards.
6. **Tests** — see Testing Strategy.
7. **Docs** — note the hover preview in the detail-template section.

## Files to Modify

- `analytics-web-app/src/components/map/EventDetailContent.tsx` (new)
- `analytics-web-app/src/components/map/MapHoverTooltip.tsx` (new)
- `analytics-web-app/src/components/map/EventDetailPanel.tsx`
- `analytics-web-app/src/components/map/MapInstancedMarkers.tsx`
- `analytics-web-app/src/components/map/MapViewer.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`
- `analytics-web-app/src/components/map/__tests__/MapHoverTooltip.test.tsx` (new)
- `analytics-web-app/src/components/map/__tests__/EventDetailPanel.test.tsx` (update if needed)
- `mkdocs/docs/web-app/notebooks/cell-types.md`

## Trade-offs

- **Lift only a callback vs. lift hover state.** The mesh keeps owning
  `hoveredRowIndex` (it drives the GPU buffers); we add an outward `onHover`
  notification instead of moving state up and passing it back down. Smaller diff,
  highlight logic stays untouched (open/closed).
- **Pass `rowIndex` up, not the row object.** Lets MapCell memoize `hoveredRow`
  on the integer index, so cursor moves (x/y only) reposition without re-deriving
  the row or re-evaluating the Markdown — the throttle bounds repositions, the
  memo bounds re-evaluation.
- **rAF throttle vs. timestamp throttle.** rAF coalesces to at most one update
  per frame and naturally aligns with paint; a fixed-ms throttle would either lag
  or over-fire relative to frames. Either satisfies the issue; rAF is simpler to
  get right (no trailing-edge bookkeeping).
- **Portal + `fixed` vs. in-container `absolute`.** The cell is `overflow-hidden`,
  which would clip a cursor tooltip near the top/right edges, and a transformed
  ancestor could break `fixed`. A portal to `body` sidesteps both. The docked
  panel stays in-container because it is pinned bottom-left and never clipped.
- **Shared `EventDetailContent` vs. duplicating the Markdown block.** Extraction
  keeps the two presentations rendering byte-identical content (DRY) and means a
  future template feature lands in both at once.

## Documentation

`mkdocs/docs/web-app/notebooks/cell-types.md`, detail-template section
(`:279-320`): add a sentence that the same template is also shown as a transient
tooltip while hovering a marker (click still opens the docked panel), and that an
empty template disables the hover preview.

## Testing Strategy

- **`MapHoverTooltip.test.tsx` (new):**
  - renders the resolved template content (reuse `EventDetailPanel.test.tsx`'s
    Arrow-table/`buildRow` helpers) at the given cursor position;
  - the tooltip element is `pointer-events-none`;
  - edge-clip logic: with a stubbed `getBoundingClientRect` and a small
    `window.innerWidth/innerHeight`, assert it flips left/up rather than
    overflowing.
- **`EventDetailPanel.test.tsx`:** should still pass unchanged after the
  extraction (same rendered output); adjust only if an assertion couples to
  internal structure.
- **`MapInstancedMarkers` hover callback:** there is no R3F mount-test harness
  today (`MapViewer.test.tsx` only covers pure `cameraBasisFromSpherical` math,
  and `@react-three/test-renderer` is not a dependency), so cover the `onHover`
  contract via the `MapHoverTooltip` + MapCell wiring tests and verify
  interactively. (If a true mesh unit test is wanted, add
  `@react-three/test-renderer` as a new dev dependency and assert `onHover` fires
  with the instance id on over and `null` on out.)
- **Full:** `yarn lint`, `yarn type-check`, `yarn test` from `analytics-web-app/`.
- **Manual:** hover markers → tooltip follows cursor and shows the same content
  as click; near cell/screen edges it flips and stays on-screen; clearing the
  template removes the tooltip; click still opens the docked panel.

## Open Questions

- **Cursor offset / flip thresholds** (`14px`, viewport margin) are suggested
  defaults — easy to tune during implementation.
