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
```

- `[label](url)` — Markdown link syntax
- `$row.column_name` — Access any column value from the current row

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
│ Row data: $row.column_name      │
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
  format: string      // Markdown format string with $row.x macros
}
```

### Step 2: Create Macro Expander

**File:** `analytics-web-app/src/lib/format-utils.ts` (new)

```typescript
const ROW_MACRO_REGEX = /\$row\.(\w+)/g

/**
 * Expand $row.column_name macros using row data
 */
export function expandRowMacros(
  template: string,
  row: Record<string, unknown>
): string {
  return template.replace(ROW_MACRO_REGEX, (_, columnName) => {
    const value = row[columnName]
    return value != null ? String(value) : ''
  })
}
```

### Step 3: Create Markdown Link Parser

**File:** `analytics-web-app/src/lib/format-utils.ts`

```typescript
const MARKDOWN_LINK_REGEX = /\[([^\]]+)\]\(([^)]+)\)/g

interface TextSegment {
  type: 'text' | 'link'
  text: string
  href?: string
}

/**
 * Parse markdown links in text, returns segments for rendering
 */
export function parseMarkdownLinks(text: string): TextSegment[] {
  const segments: TextSegment[] = []
  let lastIndex = 0

  for (const match of text.matchAll(MARKDOWN_LINK_REGEX)) {
    // Add text before this match
    if (match.index > lastIndex) {
      segments.push({ type: 'text', text: text.slice(lastIndex, match.index) })
    }
    // Add the link
    segments.push({ type: 'link', text: match[1], href: match[2] })
    lastIndex = match.index + match[0].length
  }

  // Add remaining text
  if (lastIndex < text.length) {
    segments.push({ type: 'text', text: text.slice(lastIndex) })
  }

  return segments.length ? segments : [{ type: 'text', text }]
}
```

### Step 4: Create Override Renderer

**File:** `analytics-web-app/src/lib/format-utils.ts`

```typescript
/**
 * Apply column override: expand macros, parse markdown, render links
 */
export function renderOverride(
  format: string,
  row: Record<string, unknown>
): React.ReactNode {
  const expanded = expandRowMacros(format, row)
  const segments = parseMarkdownLinks(expanded)

  if (segments.length === 1 && segments[0].type === 'text') {
    return segments[0].text
  }

  return (
    <>
      {segments.map((seg, i) =>
        seg.type === 'link' ? (
          <a
            key={i}
            href={seg.href}
            target="_blank"
            rel="noopener noreferrer"
            className="text-accent-link hover:underline"
            onClick={(e) => e.stopPropagation()}
          >
            {seg.text}
          </a>
        ) : (
          <span key={i}>{seg.text}</span>
        )
      )}
    </>
  )
}
```

### Step 5: Update TableBody to Apply Overrides

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
  const formatted = override
    ? renderOverride(override, row)
    : formatCell(value, col.type)
}
```

### Step 6: Add Overrides UI to Editor Panel

**File:** `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx`

Add collapsible sections and override editor UI:

1. Wrap existing QueryEditor in collapsible "Query" section
2. Add new collapsible "Overrides" section below
3. Override list with add/delete
4. Column dropdown (populated from query result columns)
5. Format input field

**Config schema update:**
```typescript
interface TableConfig {
  sql: string
  overrides?: ColumnOverride[]  // NEW
  // ... existing fields
}
```

### Step 7: Enable in Notebook TableCell

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
| `src/lib/format-utils.ts` | New | `expandRowMacros()`, `parseMarkdownLinks()`, `renderOverride()` |
| `src/lib/screen-renderers/table-utils.tsx` | Modify | Add `overrides` prop to TableBody, apply overrides in render |
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
[$row.exe](/process?id=$row.process_id&from=$from&to=$to)
```

Shows the exe name as the link text, includes time range from query variables.

---

## Security Notes

- `rel="noopener noreferrer"` on all links
- Links open in new tab (`target="_blank"`)
- Consider rejecting `javascript:` URLs in href

---

## Testing

1. **Manual: Table screen** — Add override, verify links render
2. **Manual: Notebook** — Same test in table cell
3. **Unit tests:**
   - `expandRowMacros()`: single macro, multiple macros, missing column → empty
   - `parseMarkdownLinks()`: no links, single link, multiple links, mixed content
   - `renderOverride()`: full integration test

---

## Future Enhancements

- Bracket notation `$row["column-name"]` for special column names
- Auto-complete for `$row.` in format input
- Preview showing rendered result for first row
