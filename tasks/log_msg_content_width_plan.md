# Log Column Content-Guided Width Plan

## Overview

Non-fixed log columns (everything except `time`, `level`, `target`) are sized to the longest value on the current page, capped at a maximum width, then truncated. `msg` is not a special case — it is simply another flex column.

## Current State (implemented)

`time`, `level`, `target` have fixed pixel widths. All other columns (`msg` and any generic columns) share the same content-driven rendering path in the `default` case of `renderLogColumn`.

## Design

### Fixed columns

| Column   | Width  |
|----------|--------|
| `time`   | 188 px |
| `level`  | 38 px  |
| `target` | 200 px |

### Flex columns

All columns with `kind === 'generic'` (including `msg`) get a content-driven width computed once per page via `computeFlexWidths`.

```ts
const FLEX_CHAR_WIDTH_PX = 7.2   // approx ch width at font-mono 12px
const MAX_FLEX_WIDTH_PX  = 700   // cap
const MIN_FLEX_WIDTH_PX  = 60    // floor
```

### `computeFlexWidths`

Exported from `log-utils.tsx`. Scans `columns` where `kind === 'generic'` over `table` rows `[startRow, endRow)` and returns `Record<string, number>` mapping column name → pixel width.

### `renderLogColumn` signature

```ts
export interface RenderLogColumnOptions {
  width?: number   // pixels; used in the default case
}

export function renderLogColumn(
  col: LogColumn,
  row: Record<string, unknown>,
  opts?: RenderLogColumnOptions,
): React.ReactNode
```

### `default` case

```tsx
default: {
  const formatted = formatCell(value, col.type)
  const w = opts?.width
  return (
    <span
      className="text-theme-text-primary mr-3 truncate"
      style={w != null
        ? { width: w, minWidth: w, maxWidth: w }
        : { minWidth: MIN_FLEX_WIDTH_PX, maxWidth: MAX_FLEX_WIDTH_PX }}
      title={formatted}
    >
      {formatted}
    </span>
  )
}
```

- `truncate` keeps rows single-line
- `title` exposes full text on hover
- When `width` is absent the span falls back to min/max bounds (backwards-compat for callers that don't compute widths)

## Files Modified

- `analytics-web-app/src/lib/screen-renderers/log-utils.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/LogRenderer.tsx`

## Callers

Both callers compute `columnWidths` via `useMemo` and pass `{ width: columnWidths[col.name] }` per cell:

**`LogCell.tsx`** — depends on `table`, `columns`, `pagination.startRow`, `pagination.endRow`

**`LogRenderer.tsx`** — depends on `resultTable`, `columns`, `numRows` (startRow = 0, endRow = numRows)

## Trade-offs

**No `msg` special case**: `msg` falls into the `default` case like any unknown column. Column ordering is preserved by schema field order; `time/level/target` are still recognised for their fixed-width rendering.

**Character width constant**: 7.2 px/ch is an overestimate for `font-mono 12px` to avoid clipping. `MAX_FLEX_WIDTH_PX` handles outliers.

**`truncate` vs `break-words`**: Bounded width keeps rows single-line. `title` covers readability.

## Testing Strategy

- Short messages (e.g., `"OK"`) → narrow column
- Long messages → capped at `MAX_FLEX_WIDTH_PX`
- Truncated messages → full text on hover via `title`
- Pagination: column width updates per page
- `LogRenderer` (standalone): same checks without pagination
