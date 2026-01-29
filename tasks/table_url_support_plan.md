# Table URL Support Implementation Plan

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
```

- `[label](url)` — Markdown link syntax
- `$row.column_name` — Access column value (alphanumeric names)
- `$row["column-name"]` — Access column value (any name, including hyphens/spaces)

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

**Mockups:** See `tasks/url_config_mockups/option_e_*.html`

---

## Implementation

### Step 1: Add Override Types

**File:** `analytics-web-app/src/lib/screen-renderers/types.ts` (or inline in table-utils)

```typescript
export interface ColumnOverride {
  column: string      // Column name to override
  format: string      // Markdown format string with $row.x or $row["x"] macros
}
```

### Step 2: Create Macro Expander

**File:** `analytics-web-app/src/lib/screen-renderers/table-utils.tsx`

```typescript
// Matches $row.columnName (dot notation for simple alphanumeric names)
const DOT_NOTATION_REGEX = /\$row\.(\w+)/g

// Matches $row["column-name"] or $row['column-name'] (bracket notation for any name)
const BRACKET_NOTATION_REGEX = /\$row\[["']([^"']+)["']\]/g

/**
 * Expand $row macros using row data.
 * Supports two syntaxes:
 * - $row.columnName (dot notation for alphanumeric column names)
 * - $row["column-name"] (bracket notation for names with hyphens, spaces, etc.)
 */
export function expandRowMacros(
  template: string,
  row: Record<string, unknown>
): string {
  // First pass: bracket notation (handles special characters)
  let result = template.replace(BRACKET_NOTATION_REGEX, (_, columnName) => {
    const value = row[columnName]
    return value != null ? String(value) : ''
  })

  // Second pass: dot notation (simple alphanumeric names)
  result = result.replace(DOT_NOTATION_REGEX, (_, columnName) => {
    const value = row[columnName]
    return value != null ? String(value) : ''
  })

  return result
}
```

### Step 3: Create Override Renderer Component

**File:** `analytics-web-app/src/lib/screen-renderers/table-utils.tsx`

Reuse the existing `react-markdown` library (already used by `MarkdownCell`) for consistent markdown rendering across the app.

```typescript
import ReactMarkdown from 'react-markdown'

interface OverrideCellProps {
  format: string
  row: Record<string, unknown>
}

/**
 * Render a column override: expand macros, then render markdown
 */
export function OverrideCell({ format, row }: OverrideCellProps) {
  const expanded = expandRowMacros(format, row)

  return (
    <ReactMarkdown
      components={{
        // Render links with proper attributes and click handling
        a: ({ href, children }) => (
          <a
            href={href}
            rel="noopener noreferrer"
            className="text-accent-link hover:underline"
          >
            {children}
          </a>
        ),
        // Strip wrapper paragraph to keep content inline
        p: ({ children }) => <>{children}</>,
      }}
    >
      {expanded}
    </ReactMarkdown>
  )
}
```

**Benefits of using react-markdown:**
- Consistent with `MarkdownCell` rendering
- Already bundled (no additional dependencies)
- Handles `javascript:` URL sanitization automatically
- Supports full markdown syntax if needed later

### Step 4: Update TableBody to Apply Overrides

**File:** `analytics-web-app/src/lib/screen-renderers/table-utils.tsx`

```typescript
export interface TableBodyProps {
  data: TableData
  columns: TableColumn[]
  compact?: boolean
  overrides?: ColumnOverride[]  // NEW
}

export function TableBody({
  data,
  columns,
  compact = false,
  overrides = []
}: TableBodyProps) {
  // Build override lookup map
  const overrideMap = useMemo(() => {
    const map = new Map<string, string>()
    for (const o of overrides) {
      map.set(o.column, o.format)
    }
    return map
  }, [overrides])

  // In render loop:
  const row = data.get(rowIdx)
  const override = overrideMap.get(col.name)
  const cellContent = override
    ? <OverrideCell format={override} row={row} />
    : formatCell(value, col.type)
}
```

### Step 5: Add Overrides UI to Editor Panel

**File:** `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx`

Add collapsible sections and override editor UI:

1. Wrap existing QueryEditor in collapsible "Query" section
2. Add new collapsible "Overrides" section below
3. Override list with add/delete
4. Column dropdown (populated from query result columns)
5. Format input field

**Config schema updates:**

Table screen config (persisted when saving screen):
```typescript
interface TableConfig {
  sql: string
  overrides?: ColumnOverride[]  // NEW - saved with screen
  // ... existing fields
}
```

Notebook cell config (persisted in notebook JSON):
```typescript
interface TableCellConfig {
  type: 'table'
  sql: string
  overrides?: ColumnOverride[]  // NEW - saved with cell
  // ... existing fields
}
```

Overrides are automatically persisted when the user saves the screen/notebook since they're part of the config object.

### Step 6: Enable in Notebook TableCell

**File:** `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx`

Pass overrides from cell config to TableBody:

```typescript
<TableBody
  data={tableData}
  columns={columns}
  compact={true}
  overrides={config.overrides}
/>
```

---

## File Changes Summary

| File | Change Type | Description |
|------|-------------|-------------|
| `src/lib/screen-renderers/table-utils.tsx` | Modify | Add `expandRowMacros()`, `OverrideCell` component, `overrides` prop to TableBody |
| `src/lib/screen-renderers/TableRenderer.tsx` | Modify | Add collapsible sections UI, override editor |
| `src/lib/screen-renderers/cells/TableCell.tsx` | Modify | Pass overrides to TableBody |
| `src/lib/screen-renderers/cells/TableCellEditor.tsx` | Modify | Add override configuration UI |

---

## Example Usage

**Query:**
```sql
SELECT
  exe,
  process_id,
  '' as link,
  start_time
FROM processes
LIMIT 20
```

**Override on `link` column:**
```
[View Process](/process?id=$row.process_id)
```

**Result:** The `link` column shows "View Process" as a clickable link for each row.

**Advanced example (dynamic label):**
```
[$row.exe](/process?id=$row.process_id)
```

Shows the exe name as the link text.

**Bracket notation example (special column names):**
```
[View](/details?id=$row["process-id"]&name=$row["Display Name"])
```

Use bracket notation for column names containing hyphens, spaces, or other special characters.

---

## Security Notes

- `rel="noopener noreferrer"` on all links (configured in custom `a` component)
- `react-markdown` sanitizes `javascript:` URLs by default

---

## Testing

1. **Manual: Table screen** — Add override, verify links render and navigate correctly
2. **Manual: Notebook** — Same test in table cell
3. **Unit tests:**
   - `expandRowMacros()`:
     - Dot notation: single macro, multiple macros
     - Bracket notation: single/double quotes, special characters in names
     - Mixed: both notations in same template
     - Missing column → empty string
     - Column name not in row → empty string
   - `OverrideCell`: renders link with correct href, handles multiple links
   - Markdown rendering already covered by existing `MarkdownCell` tests

---

## Future Enhancements

- Auto-complete for `$row.` in format input
- Preview showing rendered result for first row
