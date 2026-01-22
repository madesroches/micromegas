# Table Screen Type - Implementation Plan

## Overview

Add a new user-defined screen type called `table` to the analytics web app. This is a generic table viewer that can display results from any SQL query in a tabular format. By default, it queries the `processes` table.

## Motivation

Currently, the three existing screen types (`process_list`, `metrics`, `log`) are specialized for their specific data types. A generic `table` screen type allows users to:
- Create custom views for any SQL query result
- Explore data without needing a specialized renderer
- Build ad-hoc reports and dashboards

## Data Source

**Default Query:**
```sql
SELECT
  process_id,
  exe,
  start_time,
  last_update_time,
  username,
  computer
FROM processes
ORDER BY last_update_time DESC
LIMIT 100
```

This mirrors the `process_list` default but uses the generic table renderer instead of the specialized process list renderer.

## Architecture

### Screen Type Registration

The new screen type follows the existing pattern:

```
Frontend (React) → ScreenPage → TableRenderer
                       ↓
                   ScreenConfig (SQL + tableOptions)
                       ↓
                   Backend API → FlightSQL → Arrow Table
```

### Config Structure

```typescript
interface TableScreenConfig extends ScreenConfig {
  sql: string
  variables?: ScreenVariable[]
  tableOptions?: {
    sortColumn?: string
    sortDirection?: 'asc' | 'desc'
  }
  timeRangeFrom?: string
  timeRangeTo?: string
}
```

## Files to Create

| File | Purpose |
|------|---------|
| `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx` | Generic table renderer component |

## Files to Modify

| File | Change |
|------|--------|
| `rust/analytics-web-srv/src/screen_types.rs` | Add `Table` variant with default config |
| `analytics-web-app/src/lib/screens-api.ts` | Add `'table'` to `ScreenTypeName` type |
| `analytics-web-app/src/lib/screen-renderers/index.ts` | Register `TableRenderer` |

## Implementation Steps

### Step 1: Backend - Add Screen Type

**File:** `rust/analytics-web-srv/src/screen_types.rs`

Add new variant to the `ScreenType` enum:

```rust
pub enum ScreenType {
    ProcessList,
    Metrics,
    Log,
    Table,  // NEW
}
```

Add display name, icon, and default config:

```rust
impl ScreenType {
    pub fn display_name(&self) -> &'static str {
        match self {
            // ... existing
            Self::Table => "Table",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            // ... existing
            Self::Table => "table",
        }
    }

    pub fn default_config(&self) -> ScreenConfig {
        match self {
            // ... existing
            Self::Table => ScreenConfig {
                sql: "SELECT process_id, exe, start_time, last_update_time, username, computer FROM processes ORDER BY last_update_time DESC LIMIT 100".to_string(),
                ..Default::default()
            },
        }
    }
}
```

Add string conversion:

```rust
impl FromStr for ScreenType {
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            // ... existing
            "table" => Ok(Self::Table),
            _ => Err(/* error */),
        }
    }
}

impl Display for ScreenType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // ... existing
            Self::Table => write!(f, "table"),
        }
    }
}
```

### Step 2: Frontend - Update Types

**File:** `analytics-web-app/src/lib/screens-api.ts`

Update the type union:

```typescript
export type ScreenTypeName = 'process_list' | 'metrics' | 'log' | 'table';
```

### Step 3: Frontend - Create TableRenderer

**File:** `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx`

The renderer will:
1. Use `useStreamQuery` for Arrow data streaming
2. Auto-detect columns from Arrow schema
3. Render a generic table with sortable headers
4. Support client-side sorting (like ProcessListRenderer)

```tsx
interface TableRendererProps extends ScreenRendererProps {}

export function TableRenderer(props: TableRendererProps) {
  // ... implementation
}
```

**Key Features:**
- **Auto-column detection**: Read column names from Arrow schema
- **Type-aware formatting**: Format timestamps, numbers, strings appropriately
- **Sortable headers**: Click to sort ascending/descending
- **SQL Editor panel**: Standard right panel with QueryEditor

**Component Structure:**
```
TableRenderer
├── RendererLayout
│   ├── Left Panel (content)
│   │   ├── LoadingState / ErrorBanner / EmptyState
│   │   └── Generic Table
│   │       ├── Header Row (sortable columns)
│   │       └── Data Rows (auto-formatted cells)
│   └── Right Panel (SQL editor)
│       └── QueryEditor
```

**Cell Formatting Logic:**
```typescript
function formatCell(value: unknown, dataType: DataType): string {
  if (value === null || value === undefined) return '-';

  if (isTimeType(dataType)) {
    return formatTimestamp(value as bigint);
  }

  if (isNumericType(dataType)) {
    return formatNumber(value as number);
  }

  // Default: string representation
  return String(value);
}
```

### Step 4: Frontend - Register Renderer

**File:** `analytics-web-app/src/lib/screen-renderers/index.ts`

```typescript
import { TableRenderer } from './TableRenderer';

// In the registration section:
registerRenderer('table', TableRenderer);
```

## UI Design

### Table Layout

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  SQL Query Panel (collapsible)                                              │
├──────────────────────────────────────────────────────────┬──────────────────┤
│  ┌───────────────────────────────────────────────────┐   │  SELECT ...      │
│  │ process_id ▼ │ exe      │ start_time │ username  │   │  FROM processes  │
│  ├───────────────┼──────────┼────────────┼───────────┤   │  ORDER BY ...    │
│  │ abc123...     │ myapp    │ 2024-01-15 │ admin     │   │                  │
│  │ def456...     │ service  │ 2024-01-14 │ system    │   │  Variables:      │
│  │ ghi789...     │ worker   │ 2024-01-13 │ worker    │   │  - $begin        │
│  │ ...           │ ...      │ ...        │ ...       │   │  - $end          │
│  └───────────────┴──────────┴────────────┴───────────┘   │                  │
└──────────────────────────────────────────────────────────┴──────────────────┘
```

### Sort Indicators

- Unsorted: No indicator
- Ascending: `▲` next to column name
- Descending: `▼` next to column name

### Empty States

1. **No data**: "No results for the current query"
2. **Query error**: Error banner with SQL details

## Reference Implementation

The implementation should closely follow `ProcessListRenderer.tsx` patterns:
- Same hook usage (`useStreamQuery`)
- Same layout structure (`RendererLayout`)
- Same SQL transformation for sorting
- Same error/loading/empty state handling

Key differences from ProcessListRenderer:
- No hardcoded columns - detect from schema
- No process-specific links
- Generic cell formatting based on Arrow types

## Testing

### Manual Testing Checklist

1. **Create new table screen**
   - Navigate to `/screen/new?type=table`
   - Verify default query shows processes data
   - Verify all columns are rendered

2. **Column sorting**
   - Click column headers to sort
   - Verify sort indicator changes
   - Verify data reorders correctly

3. **Custom queries**
   - Edit SQL to query different tables
   - Verify columns update dynamically
   - Test with: `SELECT * FROM log_entries LIMIT 10`

4. **Save/Load**
   - Save screen with custom query
   - Reload page
   - Verify saved config persists

5. **Time range**
   - Change time range
   - Verify query re-executes with new range

### Edge Cases

- Query returns zero rows
- Query returns single column
- Query returns large number of columns (horizontal scroll)
- Query returns null values
- Query returns various data types (timestamps, numbers, strings, booleans)

## Future Enhancements (Out of Scope)

- Column visibility toggles
- Column reordering
- Column width persistence
- Pagination for large result sets
- Cell value links (e.g., process_id → process page)
- Export to CSV/JSON
- Column filtering

## File Changes Summary

| File | Status | Change |
|------|--------|--------|
| `rust/analytics-web-srv/src/screen_types.rs` | Modify | Add `Table` variant |
| `analytics-web-app/src/lib/screens-api.ts` | Modify | Add `'table'` to type union |
| `analytics-web-app/src/lib/screen-renderers/index.ts` | Modify | Register TableRenderer |
| `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx` | Create | New renderer component |

## Dependencies

No new dependencies required. Uses existing:
- Apache Arrow for data handling
- Existing UI components (RendererLayout, QueryEditor, etc.)
- `useStreamQuery` hook for data fetching
