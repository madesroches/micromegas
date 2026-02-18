# Resilient Log Cell Display Plan

## Overview

Make the log cell and log screen renderers display any columns returned by a query, not just the hardcoded `time`, `level`, `target`, `msg` set. When known columns are present they get their current special formatting (nanosecond timestamps, level color-coding, truncated target). Unknown columns render using the existing `formatCell()` utility from `table-utils.tsx`. Missing expected columns are simply omitted.

Issue: #826

## Current State

Both `LogRenderer.tsx` (screen mode) and `LogCell.tsx` (notebook cell mode) share the same problem:

1. **Hardcoded `LogRow` interface** — `LogCell.tsx:71-76`, `LogRenderer.tsx:62-67`:
   ```typescript
   interface LogRow {
     time: unknown
     level: string
     target: string
     msg: string
   }
   ```

2. **Hardcoded column extraction** — rows are built by accessing `row.time`, `row.level`, `row.target`, `row.msg` directly from Arrow table results, ignoring any other columns.

3. **Hardcoded rendering** — four fixed `<span>` elements with fixed widths:
   - `time`: 188px, nanosecond-formatted timestamp
   - `level`: 38px, color-coded by level name
   - `target`: 200px, truncated with tooltip
   - `msg`: flex-1, word-wrapped

If the query returns extra columns (e.g., `process_id`, `thread_id`) or different columns entirely, they are silently dropped. If expected columns are missing, the code renders empty strings (tolerant via `??` but wastes space on blank columns).

### Existing pattern to follow

`TableRenderer.tsx:366-372` discovers columns dynamically from the Arrow schema:
```typescript
const allColumns = table.schema.fields.map((field) => ({
  name: field.name,
  type: field.type,
}))
```

`table-utils.tsx` provides `formatCell(value, dataType)` for type-aware formatting, and `TableColumn` / `TableBody` for generic table rendering.

## Design

### Approach: hybrid rendering

Keep the log-specific visual style (dense monospace rows, level colors, nanosecond timestamps) but discover columns from the schema rather than hardcoding them.

**Column classification at render time:**

| Column name | If present | Rendering |
|---|---|---|
| `time` | Special: `formatLocalTime()` with nanosecond precision, fixed 188px |
| `level` | Special: numeric→name mapping, color-coded, fixed 38px |
| `target` | Special: truncated with tooltip, fixed 200px |
| `msg` | Special: flex-1, word-wrapped (always last among known columns) |
| anything else | Generic: `formatCell(value, dataType)` from table-utils, auto-width |

**Column ordering:** known columns appear first in their canonical order (time, level, target, msg), followed by extra columns in schema order. Columns not present in the schema are simply skipped.

**No new configuration surface** — this is purely about resilience. The log renderer doesn't need column management (hide/sort/override) that the table renderer has.

### Data flow change

Before:
```
Arrow Table → extract 4 hardcoded fields → LogRow[] → render 4 spans
```

After:
```
Arrow Table → read schema.fields → classify columns → render per-column spans
```

We stop converting to an intermediate `LogRow[]` array. Instead, iterate over Arrow table rows directly and render each column based on its classification.

### Shared code

Extract the column classification logic and the per-column render function into a shared module so both `LogRenderer.tsx` and `LogCell.tsx` use the same code. This avoids the current duplication of `LogRow`, `formatLocalTime`, `getLevelColor`, and `LEVEL_NAMES` between the two files.

New file: `analytics-web-app/src/lib/screen-renderers/log-utils.ts`

Contents:
- `LEVEL_NAMES` constant (moved from both files)
- `formatLocalTime()` (moved from both files)
- `getLevelColor()` (moved from both files)
- `formatLevelValue()` — normalizes numeric or string level values
- `classifyLogColumns(fields: Field[]): LogColumn[]` — returns flat ordered list (known columns in canonical order, then extras in schema order)
- `LogColumn` type: `{ name: string, kind: KnownColumnName | 'generic', type: Field['type'] }`
- `KnownColumnName` type: `'time' | 'level' | 'target' | 'msg'`

## Implementation Steps

### Step 1: Create `log-utils.ts` — DONE

Created `analytics-web-app/src/lib/screen-renderers/log-utils.ts`:
- `LEVEL_NAMES` constant
- `formatLocalTime()` — nanosecond-precision timestamp formatting
- `getLevelColor()` — level name → CSS class
- `formatLevelValue()` — normalizes numeric or string level values
- `classifyLogColumns(fields: Field[]): LogColumn[]` — returns flat ordered list with `kind` discriminant
- Types: `LogColumn` with `kind: 'time' | 'level' | 'target' | 'msg' | 'generic'`, `KnownColumnName`

Unit tests (19 passing) in `analytics-web-app/src/lib/screen-renderers/__tests__/log-utils.test.ts`:
- `LEVEL_NAMES`: valid mappings + out-of-range
- `getLevelColor`: all 6 levels + unknown fallback
- `formatLevelValue`: numeric, string, null/undefined
- `formatLocalTime`: nanosecond precision, padding, falsy/unparseable
- `classifyLogColumns`: canonical ordering, extra columns in schema order, missing known columns, empty schema, type preservation

### Step 2: Update `LogCell.tsx` — DONE

- Removed duplicated `LEVEL_NAMES`, `formatLocalTime`, `getLevelColor`, `LogRow` interface
- Imports shared utils from `log-utils.ts` and `formatCell` from `table-utils.tsx`
- Added `renderLogColumn()` function that switches on `col.kind` for per-column rendering
- Replaced `useMemo` row extraction with `classifyLogColumns(data.schema.fields)`
- Iterates Arrow table rows directly via `data.get(rowIdx)`, renders columns dynamically
- Pagination unchanged, net -27 lines

### Step 3: Update `LogRenderer.tsx` — DONE

- Removed duplicated `LEVEL_NAMES`, `formatLocalTime`, `getLevelColor`, `LogRow` interface
- Imports shared utils from `log-utils.ts` and `formatCell` from `table-utils.tsx`
- Added same `renderLogColumn()` function as LogCell
- Replaced `useState<LogRow[]>` with `useState<Table | null>` — stores Arrow table directly
- Simplified row extraction `useEffect` to just `setResultTable(streamQuery.getTable())`
- Added `columns = useMemo(() => classifyLogColumns(...))` and `numRows` derived from table
- `renderContent()` iterates table rows directly with dynamic column rendering
- Net -40 lines

### Step 4: Verify — DONE

- `yarn lint`: 0 errors, 0 warnings
- `yarn test`: 716 tests passing (all 30 suites), including 19 new log-utils tests
- `yarn type-check`: only pre-existing errors in `csv-to-arrow.ts` (d3-dsv missing types), unrelated to this change
- Manual testing: TODO

## Files to Modify

| File | Action | Status |
|---|---|---|
| `analytics-web-app/src/lib/screen-renderers/log-utils.ts` | **Create** — shared log formatting utilities | DONE |
| `analytics-web-app/src/lib/screen-renderers/__tests__/log-utils.test.ts` | **Create** — unit tests for log-utils (19 tests) | DONE |
| `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx` | **Modify** — dynamic columns, import shared utils | DONE |
| `analytics-web-app/src/lib/screen-renderers/LogRenderer.tsx` | **Modify** — dynamic columns, import shared utils | DONE |

## Trade-offs

**Alternative: Reuse `TableBody` from table-utils for extra columns**
Rejected because `TableBody` renders `<table>/<tbody>/<tr>/<td>` while the log renderers use a flex-based `<div>` layout for the dense log appearance. Mixing the two would either break the visual style or require significant refactoring of the log layout. The simpler approach is to call `formatCell()` directly within the existing flex layout.

**Alternative: Convert log renderers to use `<table>` like TableRenderer**
Rejected because the flex-based dense layout is intentional for log viewing (no cell padding, compact rows, word-wrapped messages). A table layout would change the visual character of the log view.

**Alternative: Add column configuration UI (hide/sort)**
Out of scope for this issue. The goal is resilience, not a full column management feature. Can be added later if needed.

## Testing Strategy

### Unit tests — DONE

`analytics-web-app/src/lib/screen-renderers/__tests__/log-utils.test.ts` — 19 tests:
- `LEVEL_NAMES`: valid mappings (1-6) + out-of-range returns undefined
- `getLevelColor`: all 6 standard levels return distinct classes + unknown fallback
- `formatLevelValue`: numeric→name, out-of-range→UNKNOWN, string passthrough, null/undefined→empty
- `formatLocalTime`: falsy→29-char padded empty, nanosecond extraction, short fractional padding, no-fractional zeros, unparseable→padded empty
- `classifyLogColumns`: canonical ordering regardless of schema order, extras appended in schema order, no-known-columns case, subset of known columns, empty schema, type preservation

### Automated checks — DONE

- `yarn lint`: clean
- `yarn test`: 716/716 passing
- `yarn type-check`: only pre-existing errors in csv-to-arrow.ts (unrelated)

### Manual tests — TODO

1. **Manual test — default query**: standard `log_entries` query should look identical to current behavior
4. **Manual test — extra columns**: query like `SELECT time, level, target, msg, process_id FROM log_entries` should show the extra column
5. **Manual test — missing columns**: query like `SELECT time, msg FROM log_entries` should show only time and msg, no errors
6. **Manual test — no known columns**: query like `SELECT count(*) as cnt FROM log_entries` should display the result in generic format
7. **Manual test — LogCell in notebook**: same scenarios in notebook cell mode
