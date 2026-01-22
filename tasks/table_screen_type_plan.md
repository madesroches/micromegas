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
$order_by
LIMIT 100
```

This mirrors the `process_list` default but uses the generic table renderer instead of the specialized process list renderer.

**Available Variables:**
| Variable | Description |
|----------|-------------|
| `$begin` | Time range start (ISO timestamp) |
| `$end` | Time range end (ISO timestamp) |
| `$order_by` | Full ORDER BY clause, e.g., `ORDER BY last_update_time DESC` or empty string (controlled by column header clicks) |

The `$order_by` macro includes the `ORDER BY` keywords, expanding to either `ORDER BY column_name ASC/DESC` or an empty string when no sort is active. This allows users to control where sorting is applied in their query, which works cleanly with CTEs and subqueries.

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

The table screen uses the base `ScreenConfig` with additional sort fields:

```typescript
// In screens-api.ts, ScreenConfig already has sql, variables, timeRangeFrom, timeRangeTo
// Add these optional fields for table screens:
export interface ScreenConfig {
  // ... existing fields
  sortColumn?: string      // Column name for sorting
  sortDirection?: 'asc' | 'desc'  // Sort direction
}
```

**Persisted state:**
- `sortColumn`: Current sort column name, or `null`/`undefined` for no sorting (default: no sort)
- `sortDirection`: `'asc'` | `'desc'` (default: `'asc'` when a column is first selected)

## Files to Create

| File | Purpose |
|------|---------|
| `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx` | Generic table renderer component |

## Files to Modify

| File | Change |
|------|--------|
| `rust/analytics-web-srv/src/screen_types.rs` | Add `Table` variant with default config, update error message |
| `analytics-web-app/src/lib/screens-api.ts` | Add `'table'` to `ScreenTypeName`, add `sortColumn` and `sortDirection` to `ScreenConfig` |
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
            Self::Table => serde_json::json!({
                "sql": "SELECT process_id, exe, start_time, last_update_time, username, computer\nFROM processes\n$order_by\nLIMIT 100",
                "variables": []
            }),
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
4. Use `$order_by` variable substitution for sorting (like `ProcessesPage`)

```tsx
export function TableRenderer(props: TableRendererProps) {
  // ... implementation
}
```

**Key Features:**
- **Auto-column detection**: Read column names from Arrow schema
- **Type-aware formatting**: Format timestamps, numbers, strings, booleans appropriately
- **Sortable headers**: Click to sort ascending/descending
- **SQL Editor panel**: Standard right panel with QueryEditor

**Variables for QueryEditor:**
```typescript
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
  { name: 'order_by', description: 'ORDER BY clause or empty (click headers to cycle: none → ASC → DESC → none)' },
]
```

**Sorting via `$order_by` Substitution:**

Sort state is read from config (persisted). When no sort is active, `$order_by` expands to empty string:
```typescript
const { sortColumn, sortDirection } = config

const orderByValue = sortColumn
  ? `ORDER BY ${sortColumn} ${sortDirection?.toUpperCase() ?? 'ASC'}`
  : ''

const executeQuery = useCallback((sql: string) => {
  streamQuery.execute({
    sql,
    params: {
      begin: timeRange.begin,
      end: timeRange.end,
      order_by: orderByValue,
    },
    begin: timeRange.begin,
    end: timeRange.end,
  })
}, [orderByValue, timeRange])
```

**Three-state sort cycling:**

Column headers cycle through: no sort → ASC → DESC → no sort

```typescript
const handleSort = (columnName: string) => {
  if (sortColumn !== columnName) {
    // New column: start with ASC
    onConfigChange({ ...config, sortColumn: columnName, sortDirection: 'asc' })
  } else if (sortDirection === 'asc') {
    // ASC → DESC
    onConfigChange({ ...config, sortDirection: 'desc' })
  } else {
    // DESC → no sort (clear)
    onConfigChange({ ...config, sortColumn: undefined, sortDirection: undefined })
  }
  onUnsavedChange()
}
```

**Component Structure:**
```
TableRenderer
├── RendererLayout
│   ├── Left Panel (content)
│   │   ├── LoadingState / ErrorBanner / EmptyState
│   │   └── Generic Table
│   │       ├── Header Row (sortable columns from Arrow schema)
│   │       └── Data Rows (auto-formatted cells)
│   └── Right Panel (SQL editor)
│       └── QueryEditor (with $order_by in currentValues)
```

**Cell Formatting Logic:**
```typescript
import { DataType } from 'apache-arrow'

function formatCell(value: unknown, dataType: DataType): string {
  if (value === null || value === undefined) return '-'

  if (DataType.isTimestamp(dataType)) {
    return formatTimestamp(value as bigint)
  }

  if (DataType.isInt(dataType) || DataType.isFloat(dataType)) {
    return typeof value === 'number' ? value.toLocaleString() : String(value)
  }

  if (DataType.isBool(dataType)) {
    return value ? 'true' : 'false'
  }

  return String(value)
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
│  │ process_id   │ exe ▼    │ start_time │ username  │   │  FROM processes  │
│  ├───────────────┼──────────┼────────────┼───────────┤   │  $order_by       │
│  │ abc123...     │ myapp    │ 2024-01-15 │ admin     │   │  LIMIT 100       │
│  │ def456...     │ service  │ 2024-01-14 │ system    │   │                  │
│  │ ghi789...     │ worker   │ 2024-01-13 │ worker    │   │  Variables:      │
│  │ ...           │ ...      │ ...        │ ...       │   │  $order_by = ORDER BY exe DESC │
│  └───────────────┴──────────┴────────────┴───────────┘   │  $begin = 2024-...│
│                                                          │  $end = 2024-... │
└──────────────────────────────────────────────────────────┴──────────────────┘
```

### Sort Indicators (3-state cycle)

Clicking a column header cycles through: **no sort → ASC → DESC → no sort**

- **No sort**: No indicator (default state)
- **Ascending**: `▲` next to column name
- **Descending**: `▼` next to column name

### Empty States

1. **No data**: "No results for the current query"
2. **Query error**: Error banner with SQL details

## Reference Implementation

The implementation should follow patterns from both:

**From `ProcessesPage.tsx`:**
- `$order_by` variable substitution (not SQL regex manipulation)
- Variable passing to `useStreamQuery`

**From `ProcessListRenderer.tsx`:**
- Hook usage (`useStreamQuery`)
- Layout structure (`RendererLayout`)
- Error/loading/empty state handling
- `useSqlHandlers` and `useTimeRangeSync` hooks

**Key differences from ProcessListRenderer:**
- No hardcoded columns - detect from Arrow schema
- No process-specific links
- Generic cell formatting based on Arrow `DataType`
- Sort state persisted to config (not local state)

## Testing

### Manual Testing Checklist

1. **Create new table screen**
   - Navigate to `/screen/new?type=table`
   - Verify default query shows processes data
   - Verify all columns are rendered

2. **Column sorting (3-state cycle)**
   - Click column header: no sort → ASC (▲ indicator)
   - Click same column again: ASC → DESC (▼ indicator)
   - Click same column again: DESC → no sort (no indicator)
   - Verify data reorders correctly at each state
   - Verify `$order_by` value updates: `ORDER BY col ASC` → `ORDER BY col DESC` → empty

3. **Custom queries**
   - Edit SQL to query different tables
   - Verify columns update dynamically
   - Test with: `SELECT * FROM log_entries LIMIT 10`

4. **Save/Load**
   - Save screen with custom query and sort settings
   - Reload page
   - Verify saved config persists (SQL, sort column, sort direction)

5. **Time range**
   - Change time range
   - Verify query re-executes with new range

### Edge Cases

- Query returns zero rows
- Query returns single column
- Query returns large number of columns (horizontal scroll)
- Query returns null values
- Query returns various data types (timestamps, numbers, strings, booleans)
- Query without `$order_by` macro (clicking sort headers updates state but has no effect on query results)
- Query with `$order_by` in subquery or CTE
- Saved sortColumn no longer exists in modified query (query fails with clear error, user can clear sort)

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
| `rust/analytics-web-srv/src/screen_types.rs` | Modify | Add `Table` variant, update error message |
| `analytics-web-app/src/lib/screens-api.ts` | Modify | Add `'table'` to type union, add `sortColumn`/`sortDirection` to `ScreenConfig` |
| `analytics-web-app/src/lib/screen-renderers/index.ts` | Modify | Register TableRenderer |
| `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx` | Create | New renderer with `$order_by` substitution |

## Dependencies

No new dependencies required. Uses existing:
- Apache Arrow for data handling
- Existing UI components (RendererLayout, QueryEditor, etc.)
- `useStreamQuery` hook for data fetching
