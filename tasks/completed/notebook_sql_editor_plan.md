# Notebook SQL Editor Improvements Plan

## Overview

Improve the SQL editing experience in notebook cells by: (1) allowing lines to extend horizontally instead of wrapping, (2) adding automatic SQL formatting like the Grafana plugin, and (3) supporting Ctrl+Enter to run the query.

## Current State

The SQL editor is `SyntaxEditor` (`analytics-web-app/src/components/SyntaxEditor.tsx`), a dual-layer component: a transparent `<textarea>` over a syntax-highlighted `<pre>`. Key issues:

- **Line wrapping**: The `<pre>` uses `whitespace-pre-wrap break-words` (line 162), forcing long SQL lines to wrap. The textarea has no `white-space` override, so it follows default wrapping behavior.
- **No SQL formatting**: The Grafana plugin uses `sql-formatter-plus` (`grafana/src/components/sqlFormatter.ts`) but the analytics web app has no formatter.
- **No keyboard shortcuts**: Neither `SyntaxEditor` nor `CellEditor` handles Ctrl+Enter. The `CellEditorProps` interface (`cell-registry.ts:69`) doesn't include `onRun`.
- **No `onRun` in cell editors**: The Run button lives in `CellEditor.tsx:152`, not accessible from type-specific editors like `TableCellEditor`.

## Design

### 1. Horizontal Scrolling in SyntaxEditor

Change the `<pre>` and `<textarea>` from wrapping to horizontal scroll **only when `language === 'sql'`**. Markdown editing must keep word wrapping (MarkdownCell uses SyntaxEditor with `language="markdown"`).

- `<pre>`: When SQL, use `whitespace-pre` and keep `overflow-hidden` (instead of `whitespace-pre-wrap break-words overflow-hidden`). Keep `pointer-events-none` so scrolling is driven by the textarea. The existing `scrollLeft` sync sets `preRef.scrollLeft` programmatically, which works on `overflow-hidden` elements — no scrollbar needed on the pre. When markdown, keep existing `whitespace-pre-wrap break-words overflow-hidden`.
- `<textarea>`: When SQL, add `style={{ whiteSpace: 'pre' }}` to prevent wrapping. When markdown, leave default wrapping.
- Both layers must scroll-sync horizontally (already handled by the existing `handleScroll` callback which syncs `scrollLeft`).

The container `<div>` already has `overflow-hidden` — keep the container as-is since the absolute-positioned children handle their own overflow.

### 2. SQL Format Button and Toolbar

Add `sql-formatter-plus` as a dependency (same package the Grafana plugin uses).

Create `analytics-web-app/src/lib/sqlFormatter.ts` mirroring the Grafana implementation:
```typescript
import sqlFormatter from 'sql-formatter-plus'

export function formatSQL(sql: string): string {
  return sqlFormatter.format(sql).replace(/(\$ \{ .* \})|(\$ __)|(\$ \w+)/g, (m: string) => {
    return m.replace(/\s/g, '')
  })
}
```

The post-processing regex preserves `$variable` references that the formatter breaks apart with spaces.

Add a toolbar row below the editor (similar to the Grafana plugin's `QueryTool` component at `grafana/src/components/QueryTool.tsx`). The toolbar contains:
- **Format button** (`{}` icon) — formats the SQL
- **Ctrl+Enter hint** (keyboard icon + tooltip) — tells users about the run shortcut. Only shown when `onRunShortcut` is provided, so the hint never appears without a functioning shortcut.

The toolbar is part of `SyntaxEditor` and shown automatically when `language === 'sql'`. It sits below the editor area inside the same border, as a small row with `text-xs` controls. The format button always appears for SQL; the keyboard hint is conditional on `onRunShortcut` being provided.

### 3. Ctrl+Enter to Run

**Approach**: Add an `onRunShortcut` callback prop to `SyntaxEditor`. When the textarea receives `keydown` with `Ctrl+Enter` (or `Cmd+Enter` on Mac), call the callback.

**Wiring**: Thread `onRun` from `CellEditor` into the type-specific editors:
1. Extend `CellEditorProps` (`cell-registry.ts`) with `onRun?: () => void`
2. `CellEditor.tsx` passes `onRun` to `meta.EditorComponent`
3. `HorizontalGroupCell.tsx`'s `ChildEditorView` also renders `meta.EditorComponent` — pass its existing `onRun` prop through as well
4. Each cell editor that uses `SyntaxEditor` passes `onRun` as `onRunShortcut`

This keeps the shortcut scoped to when the SQL editor is focused.

## Implementation Steps

### Step 1: Add `sql-formatter-plus` dependency
- `cd analytics-web-app && yarn add sql-formatter-plus`
- Add type declaration if needed (the Grafana plugin uses `// @ts-ignore`)

### Step 2: Create SQL formatter utility
- New file: `analytics-web-app/src/lib/sqlFormatter.ts`
- Same implementation as Grafana plugin's `formatSQL`

### Step 3: Update SyntaxEditor for horizontal scrolling (SQL only)
- Make `<pre>` classes conditional on `language`:
  - SQL: replace `whitespace-pre-wrap break-words` with `whitespace-pre` (keep `overflow-hidden`; programmatic `scrollLeft` sync handles scrolling)
  - Markdown: keep existing `whitespace-pre-wrap break-words overflow-hidden`
- Make `<textarea>` wrapping conditional on `language`:
  - SQL: add `style={{ whiteSpace: 'pre' }}`
  - Markdown: no style override (keeps default wrapping)
- Verify scroll sync still works (it should — `scrollLeft` sync already exists)

### Step 4: Add toolbar with format button and keyboard hint to SyntaxEditor
- Add prop: `onRunShortcut?: () => void`
- When `language === 'sql'`, automatically render a toolbar row below the editor area (inside the border container)
- Toolbar contains: Format button (brackets-curly / `WrapText` icon from lucide), keyboard hint icon with "Ctrl+Enter to run" tooltip (only when `onRunShortcut` is provided)
- Format button calls `formatSQL(value)` and passes result to `onChange`
- Import `formatSQL` from `@/lib/sqlFormatter`
- Restructure the component: container holds the editor area (relative, flex-1) + toolbar row (flex, no-shrink)

### Step 5: Add Ctrl+Enter support to SyntaxEditor
- Add `onKeyDown` handler to the textarea
- Detect `(e.ctrlKey || e.metaKey) && e.key === 'Enter'`
- Call `onRunShortcut?.()` and `e.preventDefault()`

### Step 6: Thread `onRun` through CellEditorProps
- Add `onRun?: () => void` to `CellEditorProps` in `cell-registry.ts`
- In `CellEditor.tsx`, pass `onRun` to `meta.EditorComponent`
- In `HorizontalGroupCell.tsx`, pass `onRun` from `ChildEditorView` to `meta.EditorComponent`

### Step 7: Update cell editors to use new SyntaxEditor props
- `TableCell.tsx`, `LogCell.tsx`, `ChartCell.tsx`, `TransposedTableCell.tsx`, `PropertyTimelineCell.tsx`, `SwimlaneCell.tsx`: add `onRunShortcut={onRun}` to their `<SyntaxEditor>` calls (toolbar shows automatically for SQL)
- `VariableCell.tsx`: add `onRunShortcut={onRun}` when in SQL/combobox mode
- `MarkdownCell.tsx`: skip (markdown, not SQL — no toolbar rendered)

### Step 8: Update QueryEditor (non-notebook SQL panel)
- `QueryEditor.tsx` also uses `SyntaxEditor` — wire `onRunShortcut` to its `handleRun` (toolbar shows automatically)

### Step 9: Update TableRenderer (standalone SQL renderer)
- `TableRenderer.tsx` also uses `SyntaxEditor` directly (line 270) — wire `onRunShortcut` to its `handleSqlRun` callback

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/package.json` | Add `sql-formatter-plus` dependency |
| `analytics-web-app/src/lib/sqlFormatter.ts` | New file — format utility |
| `analytics-web-app/src/components/SyntaxEditor.tsx` | Horizontal scroll, format button, Ctrl+Enter |
| `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` | Add `onRun` to `CellEditorProps` |
| `analytics-web-app/src/components/CellEditor.tsx` | Pass `onRun` to EditorComponent |
| `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` | Pass `onRun` from ChildEditorView to EditorComponent |
| `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` | Wire new props |
| `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx` | Wire new props |
| `analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx` | Wire new props |
| `analytics-web-app/src/lib/screen-renderers/cells/TransposedTableCell.tsx` | Wire new props |
| `analytics-web-app/src/lib/screen-renderers/cells/PropertyTimelineCell.tsx` | Wire new props |
| `analytics-web-app/src/lib/screen-renderers/cells/SwimlaneCell.tsx` | Wire new props |
| `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx` | Wire new props (if SQL mode) |
| `analytics-web-app/src/components/QueryEditor.tsx` | Wire new props |
| `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx` | Wire `onRunShortcut` to `handleSqlRun` |

## Testing Strategy

- Build: `cd analytics-web-app && yarn build`
- Manual: open a notebook, verify SQL lines don't wrap and scroll horizontally
- Manual: click Format button, verify SQL gets formatted and `$variables` preserved
- Manual: press Ctrl+Enter in SQL editor, verify query runs
- Run existing tests: `cd analytics-web-app && yarn test`
