# Log Msg Column Content-Guided Width Plan

## Overview

The `msg` column in log cells currently uses `flex-1`, expanding to fill all horizontal space regardless of message length. This wastes space when messages are short and causes multi-line wrapping when messages are long. Instead, size the column to the longest `msg` value on the current page, capped at a maximum width, then truncate overflow.

## Current State

`renderLogColumn` in `log-utils.tsx:109` renders the `msg` case as:

```tsx
case 'msg':
  return (
    <span className="text-theme-text-primary flex-1 break-words">{String(value ?? '')}</span>
  )
```

`flex-1` makes it consume all remaining space after the fixed-width columns (time: 188px, level: 38px, target: 200px). `break-words` causes multi-line rows for long messages.

Both consumers call `renderLogColumn` the same way:
- `LogCell.tsx:67` (notebook log cell, paginates — relevant page is `startRow..endRow`)
- `LogRenderer.tsx` (standalone renderer — renders `numRows` rows from the result table)

`renderLogColumn` signature: `(col: LogColumn, row: Record<string, unknown>): React.ReactNode`

## Design

### Width Computation

Add a `msgWidth` option to `renderLogColumn`. Callers compute it from the visible page rows:

```ts
const MSG_CHAR_WIDTH_PX = 7.2   // approx ch width at font-mono 12px
const MAX_MSG_WIDTH_PX  = 700   // cap — prevents runaway columns
const MIN_MSG_WIDTH_PX  = 120   // floor — avoids collapsing on empty pages
```

```ts
const msgWidth = useMemo(() => {
  if (!table) return MIN_MSG_WIDTH_PX
  let max = 0
  for (let i = startRow; i < endRow; i++) {
    const row = table.get(i)
    const len = String(row?.msg ?? '').length
    if (len > max) max = len
  }
  return Math.min(Math.max(Math.ceil(max * MSG_CHAR_WIDTH_PX), MIN_MSG_WIDTH_PX), MAX_MSG_WIDTH_PX)
}, [table, startRow, endRow])
```

For `LogRenderer` (no pagination), `startRow = 0`, `endRow = numRows`.

### `renderLogColumn` Signature Change

```ts
export interface RenderLogColumnOptions {
  msgWidth?: number   // pixels; if omitted, falls back to flex-1
}

export function renderLogColumn(
  col: LogColumn,
  row: Record<string, unknown>,
  opts?: RenderLogColumnOptions,
): React.ReactNode
```

### `msg` Case

```tsx
case 'msg': {
  const w = opts?.msgWidth
  return (
    <span
      className="text-theme-text-primary mr-3 truncate"
      style={w != null ? { width: w, minWidth: w, maxWidth: w } : undefined}
      title={String(value ?? '')}
    >
      {String(value ?? '')}
    </span>
  )
}
```

- `truncate` (width path) keeps rows single-line within the fixed column width
- `title` on the span exposes full text on hover (consistent with `target` column)
- When `msgWidth` is not provided the span falls back to `flex-1 break-words`, preserving existing behavior for all current callers

Without `flex-1`, the span won't expand at all if no width is given. Use `break-words` in the fallback to keep backward compatibility:

```tsx
className={w != null
  ? 'text-theme-text-primary mr-3 truncate'
  : 'text-theme-text-primary flex-1 break-words'}
style={w != null ? { width: w, minWidth: w, maxWidth: w } : undefined}
```

## Implementation Steps

1. **`log-utils.tsx`** — add `RenderLogColumnOptions` interface and `msgWidth` param to `renderLogColumn`; update `msg` case to use width + truncate as described above. The three constants (`MSG_CHAR_WIDTH_PX`, `MAX_MSG_WIDTH_PX`, `MIN_MSG_WIDTH_PX`) are file-private; do not export them.

2. **`LogCell.tsx`** — compute `msgWidth` via `useMemo` over `table`, `pagination.startRow`, `pagination.endRow`; pass `{ msgWidth }` as the third arg to `renderLogColumn`.

3. **`LogRenderer.tsx`** — compute `msgWidth` via `useMemo` over `resultTable` and `numRows`; pass `{ msgWidth }` as the third arg to `renderLogColumn`.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/log-utils.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/LogRenderer.tsx`

## Trade-offs

**Content-scan vs CSS `ch` units**: Pure CSS can't measure max content width across rows. A JS scan at render time is the right tool; it's O(page-size) over already-materialized Arrow rows.

**Character width constant**: A fixed `7.2px/ch` is an approximation for `font-mono` at `12px`. This is slightly over rather than under to avoid clipping edge cases. The `MAX_MSG_WIDTH_PX` cap handles outliers.

**`truncate` vs `break-words`**: With a bounded width, `truncate` keeps rows single-line, matching the compact log style. The `title` attribute covers readability. If wrap behavior is ever desired it can be added per-row as a separate feature.

## Testing Strategy

- Load a log cell with short messages (e.g., `"OK"`) — column should be narrow.
- Load a log cell with long messages — column should cap at `MAX_MSG_WIDTH_PX`.
- Verify truncated messages show full text on hover via `title`.
- Pagination: change page and verify the column width updates to reflect the new page's content.
- `LogRenderer` (standalone screen): same checks without pagination.
