# Fix $from/$to Macros in Column Overrides (Issue #914)

## Overview

Column override format strings accept `$begin` and `$end` as built-in macros (they pass validation in `BUILTIN_MACROS`), but they are never substituted when expanding the override format string. Additionally, these macros should be renamed to `$from` and `$to` to match the user-facing URL parameter names.

## Current State

**The bug:** In `OverrideCell` (`table-utils.tsx:215-246`), the expansion pipeline calls `expandVariableMacros(format, variables)` then `expandRowMacros(...)`. The `variables` object comes from `context.availableVariables` which only contains notebook Variable cell values — not the time range.

**How other cells handle it:** SQL, Markdown, and expression cells all expand `$begin`/`$end` via `substituteMacros()` in `notebook-utils.ts:276-286`, which receives `timeRange` as a separate argument. Column overrides use a different expansion path (`expandVariableMacros` + `expandRowMacros`) that lacks time range awareness.

**Data flow:** `timeRange` is already available in `CellRendererProps` (cell-registry.ts:25) and is passed to `TableCell` and `ReferenceTableCell` — it just never reaches `TableBody` or `OverrideCell`.

**Naming inconsistency:** URL parameters use `from`/`to`. The internal API uses `begin`/`end`. The macros currently use `$begin`/`$end`. Users see `from`/`to` in the URL, so the macros should match.

## Design

### Rename macros from `$begin`/`$end` to `$from`/`$to`

Update the macro names everywhere they are defined and substituted. This is a breaking change for existing notebooks using `$begin`/`$end` in SQL queries.

### Pass timeRange through to OverrideCell

Thread the `timeRange` prop through `TableBody` → `OverrideCell`, then expand `$from`/`$to` during the variable expansion step.

In `expandVariableMacros` (or in `OverrideCell` directly), merge the time range values into the variables before expansion:

```typescript
// In OverrideCell, before calling expandVariableMacros:
const allVariables = { ...variables, from: timeRange.begin, to: timeRange.end }
const withVariables = expandVariableMacros(format, allVariables)
```

This reuses the existing `expandVariableMacros` machinery with no new expansion function needed.

## Implementation Steps

### 1. Rename macros in `notebook-utils.ts`

- `substituteMacros()` (line ~280): change `$begin` → `$from`, `$end` → `$to`
- `validateMacros()` (line ~409): update the skip condition from `'begin'`/`'end'` to `'from'`/`'to'`

### 2. Rename in `table-utils.tsx`

- `BUILTIN_MACROS` (line 91): change from `['row', 'begin', 'end']` to `['row', 'from', 'to']`

### 3. Rename in `notebook-expression-eval.ts`

- Update the expression context bindings where `$begin`/`$end` are documented and injected (line ~232-233)
- Update the JSDoc comment (line ~215) that documents `$begin`/`$end` as available bindings

### 4. Rename in `AvailableVariablesPanel.tsx`

- Update time variable names from `'begin'`/`'end'` to `'from'`/`'to'` (line ~52-53)

### 5. Rename in `VariableCell.tsx`

- Update expression help text that references `$begin`/`$end` bindings (line ~326)

### 6. Thread timeRange into TableBody and OverrideCell

- Add `timeRange: { begin: string; end: string }` to `TableBodyProps` (table-utils.tsx:375)
- Add `timeRange: { begin: string; end: string }` to `OverrideCellProps` (table-utils.tsx:200)
- Pass `timeRange` from `TableBody` to `OverrideCell`
- In `OverrideCell`, merge `{ from: timeRange.begin, to: timeRange.end }` into variables before calling `expandVariableMacros`

### 7. Pass timeRange when rendering TableBody and OverrideCell

- `TableCell.tsx` (~line 108): pass `timeRange` prop to `<TableBody>`
- `ReferenceTableCell.tsx` (~line 132): pass `timeRange` prop to `<TableBody>`
- `TransposedTableCell.tsx` (~line 112): pass `timeRange` directly to each `<OverrideCell>` call (TransposedTableCell renders OverrideCell directly, not via TableBody)

### 8. Update tests

- `__tests__/notebook-utils.test.ts`: rename all `$begin`/`$end` references to `$from`/`$to`
- `__tests__/table-utils.test.tsx`: update `BUILTIN_MACROS` tests, add test for `$from`/`$to` expansion in `OverrideCell`
- `cells/__tests__/MarkdownCell.test.tsx`: update time range variable references
- `__tests__/notebook-expression-eval.test.ts`: update expression context variable names

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/table-utils.tsx` — BUILTIN_MACROS, OverrideCellProps, TableBodyProps, OverrideCell
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` — substituteMacros, validateMacros
- `analytics-web-app/src/lib/screen-renderers/notebook-expression-eval.ts` — expression context bindings and JSDoc
- `analytics-web-app/src/components/AvailableVariablesPanel.tsx` — rename time variable display names from begin/end to from/to
- `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx` — update expression help text bindings
- `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` — pass timeRange to TableBody
- `analytics-web-app/src/lib/screen-renderers/cells/ReferenceTableCell.tsx` — pass timeRange to TableBody
- `analytics-web-app/src/lib/screen-renderers/cells/TransposedTableCell.tsx` — pass timeRange directly to OverrideCell calls
- `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts` — rename macros in tests
- `analytics-web-app/src/lib/screen-renderers/__tests__/table-utils.test.tsx` — rename macros, add override expansion test
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/MarkdownCell.test.tsx` — rename macros
- `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-expression-eval.test.ts` — rename macros

## Trade-offs

**Clean break on rename:** Renaming `$begin`/`$end` → `$from`/`$to` is a breaking change, but there are few existing notebooks so no aliases are needed.

**Merging into variables vs. separate expansion:** We could add a dedicated `expandTimeRangeMacros()` function, but merging into the existing variables object is simpler and reuses the existing `expandVariableMacros` function without new code paths.

## Testing Strategy

1. Run existing test suite (`yarn test`) — all renamed macro tests should pass
2. Verify `$from` and `$to` expand correctly in:
   - SQL cell queries
   - Markdown cell content
   - Column override format strings (the bug fix)
   - Expression cell evaluation
3. Verify validation still accepts `$from`/`$to` without warnings
4. Manual test: create a notebook with a table cell that has a column override using `$from` in a link, confirm it renders the time range value
