# Log Cell: One-Click Copy Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1131

## Overview

Add a clipboard icon that appears absolutely positioned within the existing left padding of each log row in `LogCell`. Clicking it copies the row as tab-separated text. No permanent space is reserved — the icon is invisible until the row is hovered.

## Current State

`LogCell.tsx` renders rows as:

```tsx
<div className={`flex px-2 py-0.5 hover:bg-app-card/50 transition-colors...`}>
  {columns.map(...renderLogColumn...)}
</div>
```

- Left padding is `px-2` (8px). That space is currently empty air.
- No clipboard capability exists; users must open devtools to extract row content.
- Existing clipboard pattern: `CopyableProcessId.tsx` — uses `Copy`/`Check` from lucide-react, `navigator.clipboard.writeText`, and a `document.execCommand('copy')` fallback. Follow the same pattern.
- `Copy` and `Check` are already available from lucide-react (used in `CopyableProcessId`).

## Design

### Icon positioning

The row div gets `position: relative` (add `relative` to its className) and `group`. The icon is `position: absolute; left: 0; top: 50%; transform: translateY(-50%)` — a 10px lucide icon. At `left: 0` with `width: 10px`, the icon extends from 0 to 10px. Row content starts at 8px (`px-2`). The rightmost 2px of the icon overlaps the first pixels of the timestamp on hover only — acceptable since:
- It is only visible during hover
- 2px is visually negligible
- The timestamp's leading pixels (year digits) are low-information

No padding change. Zero permanent space used.

### Visibility

Use Tailwind `group`/`group-hover` — no JS hover state needed:

```tsx
// icon span
className="absolute left-0 top-1/2 -translate-y-1/2 opacity-0 group-hover:opacity-100 transition-opacity cursor-pointer"
```

### Copy feedback

Single `copiedRowIdx: number | null` state at the `LogCell` level (not per-row — avoids `useState` per row). On click: set `copiedRowIdx` to the absolute row index, clear it after 1.5 s via `setTimeout`. The icon for that row renders `<Check size={10} className="text-green-500" />` while it matches `copiedRowIdx`, otherwise `<Copy size={10} className="text-theme-text-muted" />`.

Reset `copiedRowIdx` to `null` on page change (add to `pagination`'s page-change handler or as a `useEffect` dependency on `pagination.currentPage`).

### Copy format

Tab-separated string using the same format functions already in `log-utils.tsx`:

```
2026-06-12 14:23:01.293847123\tINFO\tmicromegas::tracing\tStarting service at port 9000
```

One formatted value per column, joined by `\t`. Any extra generic columns are appended in schema order.

Add a pure utility to `log-utils.tsx`:

```ts
export function formatRowForCopy(columns: LogColumn[], row: Record<string, unknown>): string {
  return columns
    .map((col) => {
      switch (col.kind) {
        case 'time':   return formatLocalTime(row[col.name])
        case 'level':  return formatLevelValue(row[col.name])
        case 'target': return String(row[col.name] ?? '')
        default:       return formatCell(row[col.name], col.type)
      }
    })
    .join('\t')
}
```

### Click handler

```tsx
const handleCopyRow = useCallback(
  async (rowIdx: number, text: string, e: React.MouseEvent) => {
    e.stopPropagation()
    try {
      await navigator.clipboard.writeText(text)
    } catch {
      // execCommand fallback (same pattern as CopyableProcessId)
      const ta = document.createElement('textarea')
      ta.value = text
      document.body.appendChild(ta)
      ta.select()
      document.execCommand('copy')
      document.body.removeChild(ta)
    }
    setCopiedRowIdx(rowIdx)
    setTimeout(() => setCopiedRowIdx(null), 1500)
  },
  [],
)
```

## Implementation Steps

1. **`log-utils.tsx`** — add `formatRowForCopy(columns, row)` export (pure function, no component changes).

2. **`LogCell.tsx`** — add state:
   ```tsx
   const [copiedRowIdx, setCopiedRowIdx] = useState<number | null>(null)
   ```

3. **`LogCell.tsx`** — add `handleCopyRow` callback (see above).

4. **`LogCell.tsx`** — update row div: add `relative group` to its className.

5. **`LogCell.tsx`** — render the copy icon as the first child of the row div, before the columns map:
   ```tsx
   <button
     className="absolute left-0 top-1/2 -translate-y-1/2 opacity-0 group-hover:opacity-100 transition-opacity text-theme-text-muted hover:text-theme-text-primary"
     onClick={(e) => handleCopyRow(rowIdx, formatRowForCopy(columns, row), e)}
     aria-label="Copy row"
     tabIndex={-1}
   >
     {copiedRowIdx === rowIdx
       ? <Check size={10} className="text-green-500" />
       : <Copy size={10} />}
   </button>
   ```

6. **`LogCell.tsx`** — reset `copiedRowIdx` on page change:
   ```tsx
   useEffect(() => { setCopiedRowIdx(null) }, [pagination.currentPage])
   ```

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/log-utils.tsx` — add `formatRowForCopy`
- `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx` — copy state, handler, icon

## Trade-offs

**Tab-separated vs JSON**: Tab-separated is paste-friendly in terminals, spreadsheets, and editors without extra formatting. JSON would be more structured but adds noise for the common case. Tab-separated chosen; can be extended later.

**Single `copiedRowIdx` vs per-row state**: Per-row state would require a sub-component or a `Map` and adds overhead proportional to page size. A single index at the cell level is sufficient — only one row can be "just copied" at a time.

**2px timestamp overlap on hover**: Keeping `px-2` avoids any permanent layout change. The 2px overlap is only visible during hover and on a low-information part of the timestamp. Alternative (bump to `px-3`) was rejected because it uses space when not hovering.

**`tabIndex={-1}` on the button**: The icon is a hover-only affordance; keeping it out of the tab order avoids unexpected focus behavior when navigating the log with a keyboard.

## Testing Strategy

1. `yarn type-check` — no TS errors.
2. `yarn lint` — clean.
3. `yarn test` — existing tests pass.
4. Manual in the running app (`./start_analytics_web.py`):
   - Hover a row → clipboard icon appears at the left edge.
   - Click → icon briefly turns into a green checkmark; clipboard contains tab-separated row text.
   - Navigate to another page → checkmark resets.
   - Works with all log levels and extra generic columns.
   - Non-hovered rows show no icon and no layout shift.
