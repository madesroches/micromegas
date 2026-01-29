# Table URL Support Implementation Plan

**Status: IMPLEMENTED**

## Overview

Add support for clickable URLs inside table cells in both standalone table screens and notebook screens. This enables better navigation when query results contain URLs.

**Related Issue**: [#739 - Add support for URLs inside table cells in notebook screens](https://github.com/madesroches/micromegas/issues/739)

## Design

### Approach: Column Overrides with Markdown + Macros

Instead of auto-detecting URLs, users configure **column overrides** that define how specific columns render. This is similar to Grafana's field overrides but uses markdown syntax for links.

**Override format:**
```
[View Process](/process?id=$row.process_id)
[Details](/details?id=$row["process-id"])
[$row.exe](/process?process_id=$row.process_id&from=$row.start_time&to=$row.end_time)
```

- `[label](url)` — Markdown link syntax
- `$row.column_name` — Access column value (alphanumeric names)
- `$row["column-name"]` — Access column value (any name, including hyphens/spaces)
- **Timestamps are automatically formatted as RFC3339 (ISO 8601)** for URL compatibility

### UI: Editor Panel with Collapsible Sections

The existing SQL editor panel gets a second collapsible section for overrides:

```
┌─────────────────────────────────┐
│ ▼ Query                         │
├─────────────────────────────────┤
│ SELECT                          │
│   exe,                          │
│   process_id,                   │
│   '' as link,                   │
│   start_time                    │
│ FROM processes                  │
│ LIMIT 20                        │
├─────────────────────────────────┤
│ ▼ Overrides                 [1] │
├─────────────────────────────────┤
│ ┌─────────────────────────────┐ │
│ │ Column: link            [x] │ │
│ │ Format:                     │ │
│ │ [View Process](/process?... │ │
│ └─────────────────────────────┘ │
│                                 │
│ [+ Add Override]                │
│                                 │
│ Format: [label](url)            │
│ Row data: $row.name or          │
│           $row["column-name"]   │
└─────────────────────────────────┘
```

**Behaviors:**
- Both sections collapsible (chevron + click header)
- Badge shows override count when collapsed
- Column dropdown populated from query result columns
- Query section flex-grows; Overrides section fixed height

---

## Implementation Summary

### Files Created

| File | Description |
|------|-------------|
| `src/components/OverrideEditor.tsx` | Reusable override editor component with collapsible UI, add/remove functionality, column dropdown, and format input |
| `src/lib/screen-renderers/__tests__/table-utils.test.tsx` | 28 unit tests for `expandRowMacros()` and `OverrideCell` |

### Files Modified

| File | Changes |
|------|---------|
| `src/lib/screen-renderers/table-utils.tsx` | Added `ColumnOverride` interface, `expandRowMacros()` function with RFC3339 timestamp support, `OverrideCell` component, updated `TableBody` to accept `overrides` prop |
| `src/lib/screen-renderers/TableRenderer.tsx` | Added `overrides` to `TableConfig`, custom panel with collapsible "Query" and "Overrides" sections, override management |
| `src/lib/screen-renderers/cells/TableCell.tsx` | Added overrides support in renderer and editor components |
| `src/lib/screen-renderers/cell-registry.ts` | Added `availableColumns?: string[]` to `CellEditorProps` interface |
| `src/components/CellEditor.tsx` | Added `availableColumns` prop, passed to type-specific editors |
| `src/lib/screen-renderers/NotebookRenderer.tsx` | Pass available columns from cell state to `CellEditor` |

---

## Key Implementation Details

### Macro Expansion (`expandRowMacros`)

```typescript
export function expandRowMacros(
  template: string,
  row: Record<string, unknown>,
  columnTypes?: Map<string, DataType>
): string
```

- Supports dot notation: `$row.column_name`
- Supports bracket notation: `$row["column-name"]` or `$row['column-name']`
- **Timestamp columns are automatically formatted as RFC3339** when `columnTypes` is provided
- Missing columns resolve to empty string

### Override Cell Component (`OverrideCell`)

```typescript
interface OverrideCellProps {
  format: string
  row: Record<string, unknown>
  columns: TableColumn[]  // Used for timestamp type detection
}
```

- Expands macros with proper timestamp formatting
- Renders markdown using `react-markdown`
- Links have `rel="noopener noreferrer"` for security
- Paragraph wrapper stripped for inline rendering

### Config Schema

**Table screen config:**
```typescript
interface TableConfig {
  sql: string
  overrides?: ColumnOverride[]
  sortColumn?: string
  sortDirection?: 'asc' | 'desc'
  // ... existing fields
}
```

**Notebook cell options:**
```typescript
// Overrides stored in options.overrides
interface QueryCellConfig {
  type: 'table'
  sql: string
  options?: {
    overrides?: ColumnOverride[]
    sortColumn?: string
    sortDirection?: 'asc' | 'desc'
  }
}
```

---

## Example Usage

**Query:**
```sql
SELECT
  exe,
  process_id,
  start_time,
  last_update_time,
  '' as link
FROM processes
LIMIT 20
```

**Override on `link` column:**
```
[$row.exe](/mmlocal/process?process_id=$row.process_id&from=$row.start_time&to=$row.last_update_time)
```

**Result:** The `link` column shows the exe name as a clickable link. Timestamps are automatically formatted as RFC3339:
```
/mmlocal/process?process_id=abc123&from=2024-01-15T10:30:00.000Z&to=2024-01-15T11:45:00.000Z
```

**Bracket notation example (special column names):**
```
[View](/details?id=$row["process-id"]&name=$row["Display Name"])
```

---

## Security Notes

- `rel="noopener noreferrer"` on all links (configured in custom `a` component)
- `react-markdown` sanitizes `javascript:` URLs by default

---

## Testing

### Unit Tests (28 tests in `table-utils.test.tsx`)

**`expandRowMacros` tests:**
- Dot notation: single macro, multiple macros, underscores
- Bracket notation: double quotes, single quotes, spaces, special characters
- Mixed notation in same template
- Missing columns → empty string
- Null/undefined values → empty string
- Non-string value conversion

**`expandRowMacros` timestamp tests:**
- RFC3339 formatting for timestamp columns
- Bracket notation with timestamps
- Mixed timestamp and string columns
- Backwards compatibility without column types

**`OverrideCell` tests:**
- Simple link rendering
- Dynamic link text from macros
- Multiple links
- Missing columns
- Plain text (no markdown)
- Bracket notation in links

### Manual Testing

1. **Table screen** — Add override, verify links render and navigate correctly
2. **Notebook** — Same test in table cell
3. **Timestamps** — Verify timestamp columns produce RFC3339 URLs

---

## Future Enhancements

- Auto-complete for `$row.` in format input
- Preview showing rendered result for first row
- URL encoding for special characters in values
