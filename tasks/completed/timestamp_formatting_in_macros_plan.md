# Timestamp Formatting in Macro Expansion Plan

## Issue References
- [#908](https://github.com/madesroches/micromegas/issues/908) — Hidden time columns in markdown overrides display as large int instead of RFC3339
- [#910](https://github.com/madesroches/micromegas/issues/910) — Markdown cell: `$process_info[0].start_time` renders as bigint instead of RFC3339

## Overview

Two related bugs cause timestamp values to render as raw integers (epoch nanoseconds) instead of RFC3339 strings in macro expansion contexts. Both share the same root cause: the macro expansion code paths convert Arrow timestamp values to strings via `String(value)` without checking the column's data type. The proper conversion logic (`timestampToDate()` + `.toISOString()`) already exists in `formatValueForUrl()` but is not reused by these code paths.

## Current State

### Issue #910 — Cell result references in `substituteMacros()`

In `notebook-utils.ts` line 287, cell result row values are converted with:

```typescript
return escapeSqlValue(String(row[colName]))
```

For Arrow timestamp columns, `row[colName]` is a `BigInt` (nanoseconds since epoch). `String(BigInt)` produces e.g. `"1709913600000000000"` instead of `"2024-03-08T16:00:00.000Z"`.

### Issue #908 — Hidden columns in table markdown overrides

In `table-utils.tsx`, `OverrideCell` builds a `columnTypes` map from its `columns` prop (line 215-221). However, three renderer components filter out hidden columns before passing them to `TableBody`/`OverrideCell`:

- `TableCell.tsx` (line 76): `allColumns.filter((c) => !hiddenSet.has(c.name))`
- `TableRenderer.tsx` (line 374): same filter

Note: `ReferenceTableCell.tsx` also filters hidden columns (line 101) but does not pass `overrides` to `TableBody`, so `OverrideCell` never renders there — it is **not affected**.

When a hidden time column is referenced in a markdown override, `columnTypes.get(columnName)` returns `undefined`, and `formatValueForUrl()` falls back to `String(value)`.

Note: `TransposedTableCell.tsx` is **not affected** — it passes all columns (line 48-51), not just visible ones.

### Existing timestamp conversion

`arrow-utils.ts` already provides:

- `isTimeType(dataType)` — checks `Timestamp`, `Date`, `Time` types (line 70-76)
- `timestampToDate(value, dataType)` — converts any Arrow timestamp representation to `Date` (line 53-65)
- `timestampToMs(value, dataType)` — handles BigInt unit conversion (ns/μs/ms/s) (line 13-47)

`formatValueForUrl()` in `table-utils.tsx` (line 50-65) already uses these correctly.

## Design

### Fix #910 — Add type-aware formatting to `substituteMacros()`

The cell result substitution pass has access to the Arrow `Table` object, which contains the full schema including column data types via `table.schema.fields`.

Extract a helper function (reusable by both fixes) that formats a value as RFC3339 if its column type is a time type:

```typescript
// in notebook-utils.ts
import { isTimeType, timestampToDate } from '@/lib/arrow-utils'
import type { DataType } from 'apache-arrow'

function formatArrowValue(value: unknown, dataType?: DataType): string {
  if (dataType && isTimeType(dataType)) {
    const date = timestampToDate(value, dataType)
    if (date) return date.toISOString()
  }
  return String(value)
}
```

Then in the cell result substitution pass, look up the column's data type from the table schema:

```typescript
const field = table.schema.fields.find((f) => f.name === colName)
return escapeSqlValue(formatArrowValue(row[colName], field?.type))
```

### Fix #908 — Pass all columns to `OverrideCell`

The fix is to pass `allColumns` (not `visibleColumns`) to `TableBody` as a separate prop for type resolution, while continuing to use `visibleColumns` for rendering column headers.

Add an `allColumns` prop to `TableBodyProps` and `OverrideCellProps`:

```typescript
export interface TableBodyProps {
  data: TableData
  columns: TableColumn[]       // visible columns (for rendering headers/cells)
  allColumns?: TableColumn[]   // all columns including hidden (for type resolution in overrides)
  // ... existing fields
}
```

In `OverrideCell`, use `allColumns` (falling back to `columns`) when building the `columnTypes` map:

```typescript
const columnTypes = useMemo(() => {
  const map = new Map<string, DataType>()
  for (const col of (allColumns || columns)) {
    map.set(col.name, col.type)
  }
  return map
}, [allColumns, columns])
```

In `TableCell.tsx` and `TableRenderer.tsx`, pass `allColumns` alongside the existing `visibleColumns`:

```typescript
<TableBody data={slicedData} columns={visibleColumns} allColumns={allColumns} ... />
```

## Implementation Steps

### Step 1: Add `formatArrowValue` helper to `notebook-utils.ts`

- Import `isTimeType`, `timestampToDate` from `@/lib/arrow-utils` and `DataType` from `apache-arrow`
- Add `formatArrowValue(value, dataType?)` function
- Update the cell result substitution pass (line ~285-287) to look up the column's `DataType` from `table.schema.fields` and call `formatArrowValue` instead of `String()`

### Step 2: Add tests for timestamp formatting in cell result macros

In `notebook-utils.test.ts`, add tests using an Arrow table with a `Timestamp` column:

- `$cell[0].timestamp_col` with a BigInt nanosecond value → expect RFC3339 string
- `$cell[0].timestamp_col` with a regular number value → expect RFC3339 string
- Non-time columns still use `String()` conversion

### Step 3: Pass `allColumns` through the table rendering pipeline

- Add `allColumns?: TableColumn[]` to `TableBodyProps` in `table-utils.tsx`
- Add `allColumns?: TableColumn[]` to `OverrideCellProps` in `table-utils.tsx`
- Update `OverrideCell` to use `allColumns || columns` for the `columnTypes` map
- Update `TableBody` to forward `allColumns` to `OverrideCell`
- In `TableCell.tsx`: pass `allColumns={allColumns}` to `<TableBody>`
- In `TableRenderer.tsx`: pass `allColumns={allColumns}` to `<TableBody>`

### Step 4: Add tests for hidden column type resolution

In `table-utils.test.tsx`, add a test that:

- Creates a table with a hidden timestamp column
- Configures a markdown override referencing that hidden column via `$row.timestamp_col`
- Verifies the output is RFC3339 format, not a raw integer

### Step 5: Run checks

- `yarn lint` and `yarn type-check` from `analytics-web-app/`
- `yarn test` to verify all existing + new tests pass

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` | Add `formatArrowValue` helper; use it in cell result substitution pass |
| `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts` | Add tests for timestamp formatting in `$cell[N].col` |
| `analytics-web-app/src/lib/screen-renderers/table-utils.tsx` | Add `allColumns` prop to `TableBodyProps`, `OverrideCellProps`, `OverrideCell`; forward in `TableBody` |
| `analytics-web-app/src/lib/screen-renderers/__tests__/table-utils.test.tsx` | Add test for hidden timestamp column in markdown override |
| `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` | Pass `allColumns` to `<TableBody>` |
| `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx` | Pass `allColumns` to `<TableBody>` |

## Trade-offs

### `allColumns` prop vs. separate `columnTypes` map prop

**Chosen: `allColumns` prop.** Passing the full column list preserves the existing pattern where `OverrideCell` builds its own type map. An alternative would be to pass a pre-built `Map<string, DataType>` directly, but that would diverge from the current pattern and require building the map at each call site.

### `formatArrowValue` in notebook-utils vs. reusing `formatValueForUrl` from table-utils

**Chosen: New helper in notebook-utils.** `formatValueForUrl` is tightly coupled to table rendering (returns empty string for null, designed for URL contexts). The notebook-utils context needs SQL-safe escaping applied after formatting. A small focused helper keeps the concerns separate and avoids a cross-module dependency from notebook-utils → table-utils.

## Documentation

No mkdocs changes needed — these are bug fixes to existing behavior, not new features.

## Testing Strategy

### Unit Tests
- `notebook-utils.test.ts`: Verify `$cell[0].start_time` with a `Timestamp(NANOSECOND)` column produces RFC3339 output
- `table-utils.test.tsx`: Render `OverrideCell` with `columns` containing only visible (non-timestamp) columns and `allColumns` containing all columns including a hidden timestamp column; verify `$row.timestamp_col` renders as RFC3339
- Verify existing tests continue to pass (no regressions in non-time column handling)

### Manual Testing
1. Create a notebook with a `process_info` cell querying `SELECT process_id, start_time FROM processes LIMIT 1`
2. Add a markdown cell with `Process started: $process_info[0].start_time`
3. Verify the time renders as RFC3339

4. Create a table cell with a time column, configure a column override to hide it, and reference it in a markdown override
5. Verify the time renders as RFC3339

## Open Questions

None.
