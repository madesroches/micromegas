# Log Cell Resizable Columns Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1130

## Overview

Add user-resizable columns to the log cell via draggable inline dividers between cells. Pinned widths are persisted in the cell's `options.columnWidths` object (same mechanism already used for `pageSize`). Non-pinned columns continue to auto-size from visible content. Pinned columns display an amber divider; a "Reset widths" button appears in the bottom bar when any column is pinned.

## Current State

- **`log-utils.tsx`**: `computeFlexWidths()` computes auto widths for `generic` columns from max content length on the current page. Known columns (`time`, `level`, `target`) use hardcoded Tailwind `w-[Npx] min-w-[Npx]` classes inside `renderLogColumn()`.
- **`LogCell.tsx`**: calls `computeFlexWidths`, passes `{ width }` to `renderLogColumn` for generic columns only. Known column widths are not configurable.
- **`options` persistence**: `onOptionsChange({ ...options, pageSize: size })` is the established pattern; the notebook serializes this to config and restores it on reload.

## Design

### UX (agreed in mockup review)

- Thin 5px draggable dividers between every pair of columns in every row — no header row.
- Drag a divider → pins only that column. Other columns remain auto.
- Pinned divider renders amber; auto divider renders dim gray; hovered/active renders blue.
- Right-click any divider → context menu:
  - "Reset to auto" (dimmed when not pinned)
  - "Reset all columns"
- "Reset widths" button in the cell bottom bar, visible only when ≥1 column is pinned.
- No legend.

### Data model

```ts
// stored under options.columnWidths — sparse: only pinned columns present
type ColumnWidths = Record<string, number>
```

Effective width resolution:
```
effectiveWidth(col) = pinnedWidths[col] ?? autoWidths[col] ?? MIN_COLUMN_WIDTH_PX
```

### Drag interaction

Use a `useRef` for the in-flight drag (avoids closure staleness, no re-render per pixel) and a `useState<Record<string, number>>` for `livePinnedWidths` which drives rendering:

- `mousedown` on divider → populate `dragRef`, attach document-level `mousemove`/`mouseup`.
- `mousemove` → `setLivePinnedWidths(prev => ({ ...prev, [col]: clamp(newW) }))`.
- `mouseup` → `onOptionsChange({ ...options, columnWidths: { ...livePinnedWidths } })`, detach listeners, clear `dragRef`.

Sync from outside (notebook reload): `useEffect(() => setLivePinnedWidths(pinnedWidths), [options?.columnWidths])`.

### Column widths for known columns

Remove the hardcoded Tailwind `w-[Npx] min-w-[Npx]` from `renderLogColumn` for `time`, `level`, and `target`. Extend `computeFlexWidths` to scan all columns (not just `generic` ones), so known columns get auto-sized from their formatted content just like generic columns. No fixed defaults — auto-sizing is at least as good.

### New component: `LogDivider`

Add to `log-utils.tsx`:

```tsx
interface LogDividerProps {
  col: string           // left column name
  pinned: boolean
  hovered: boolean
  onMouseDown: (e: React.MouseEvent) => void
  onContextMenu: (e: React.MouseEvent) => void
  onMouseEnter: () => void
  onMouseLeave: () => void
}
```

Renders a `<span>` (5px wide, full row height via `self-stretch`) with a 1px inner line whose color reflects state. No external CSS — inline styles or a small set of Tailwind classes.

### Context menu

Rendered as a fixed-position `<div>` inside `LogCell` (not a portal — the cell already has `overflow: auto` on the scroll container, so the menu must be outside it or use `position: fixed`). Controlled by `useState<{ col: string; x: number; y: number } | null>`. Dismissed on document `mousedown` outside.

## Implementation Steps

1. **`log-utils.tsx`** — update `computeFlexWidths`:
   - Remove the `generic`-only filter — scan all columns including `time`, `level`, `target`.
   - Use the appropriate format function per kind (`formatLocalTime`, `formatLevelValue`, `String`) to measure content length accurately.

2. **`log-utils.tsx`** — update `renderLogColumn`:
   - Remove hardcoded `w-[Npx] min-w-[Npx]` Tailwind classes from `time`, `level`, `target` cases.
   - Apply `style={{ width: opts?.width, minWidth: opts?.width }}` uniformly across all kinds (same as current generic path).

3. **`log-utils.tsx`** — add `LogDivider` component.

4. **`LogCell.tsx`** — add state:
   - `livePinnedWidths` state, `dragRef`, `hoveredDivider` state, `contextMenu` state.
   - Sync effect from `options.columnWidths`.

5. **`LogCell.tsx`** — compute effective widths from `livePinnedWidths` merged with auto widths.

6. **`LogCell.tsx`** — row rendering: insert `<LogDivider>` between each pair of columns, wiring all handlers.

7. **`LogCell.tsx`** — add context menu `<div>` (fixed position, outside scroll container).

8. **`LogCell.tsx`** — add "Reset widths" button in the bottom bar (alongside `PaginationBar`), shown only when `Object.keys(livePinnedWidths).length > 0`.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/log-utils.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx`

## Trade-offs

**Inline dividers vs header row**: Header row adds vertical space and a new DOM layer; inline dividers are invisible until hovered and keep the compact log feel. Chosen: inline dividers.

**Pin only dragged column vs snapshot all**: Snapshotting all on first drag is simpler to reason about but surprises users who only want to widen one column. Chosen: pin only the dragged column.

**`useRef` for drag state**: Avoids attaching/detaching handlers on every render during a fast drag. Width updates go through `useState` so React re-renders the rows; the ref just tracks the drag origin.

**`options.columnWidths` vs localStorage**: `options` is already persisted by the notebook layer and is per-cell, which is the right granularity. No extra persistence code needed.

## Testing Strategy

1. `yarn type-check` — no TS errors.
2. `yarn lint` — clean.
3. `yarn test` — existing tests pass.
4. Manual in the running app (`./start_analytics_web.py`):
   - Drag a divider → resizes; turns amber; other columns unchanged.
   - Page to next page → non-pinned columns reflow; pinned stays fixed.
   - Right-click pinned divider → "Reset to auto" available; resets correctly.
   - "Reset all" resets all dividers.
   - "Reset widths" button visible while pinned; disappears after reset.
   - Reload page → pinned widths restored.
