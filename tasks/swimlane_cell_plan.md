# Swimlane Cell Type Implementation Plan

## Overview

Add a new `swimlane` notebook cell type that displays horizontal swimlane visualizations with time segment bars. This generalizes the existing `ThreadCoverageTimeline` component as a reusable, SQL-driven notebook cell.

**GitHub Issue**: #763

## Background

The swimlane visualization is already implemented as `ThreadCoverageTimeline` in `analytics-web-app/src/components/ThreadCoverageTimeline.tsx`. This plan refactors that pattern into a notebook cell that can display any data matching the swimlane schema.

## Data Schema

### SQL Query Output Schema

The cell expects query results with these columns:

| Column | Type | Description |
|--------|------|-------------|
| `id` | string | Unique identifier for the lane |
| `name` | string | Display name for the lane |
| `begin` | timestamp | Segment start time |
| `end` | timestamp | Segment end time |

Multiple rows with the same `id`/`name` create multiple segments in a single lane.

**Lane ordering**: Lanes appear in order of first occurrence in query results. Use `ORDER BY` in SQL to control lane order.

### Example Query

```sql
SELECT
  stream_id as id,
  property_get("streams.properties", 'thread-name') as name,
  begin_time as begin,
  end_time as end
FROM blocks
WHERE process_id = '$process_id'
  AND array_has("streams.tags", 'cpu')
ORDER BY name, begin
```

## Files to Create

| File | Purpose |
|------|---------|
| `analytics-web-app/src/lib/screen-renderers/cells/SwimlaneCell.tsx` | Cell renderer, editor, and metadata |

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/src/lib/screen-renderers/notebook-types.ts` | Add `'swimlane'` to `CellType` union |
| `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` | Import and register `swimlaneMetadata` |
| `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` | Add default SQL template for swimlane |

## Implementation Steps

### Step 1: Add Type Definition

**File**: `notebook-types.ts`

Update the `CellType` union:

```typescript
export type CellType = 'table' | 'chart' | 'log' | 'markdown' | 'variable' | 'propertytimeline' | 'swimlane'
```

Update `QueryCellConfig.type`:

```typescript
export interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log' | 'propertytimeline' | 'swimlane'
  sql: string
  options?: Record<string, unknown>
}
```

### Step 2: Create SwimlaneCell Component

**File**: `cells/SwimlaneCell.tsx`

Structure based on `ThreadCoverageTimeline` with these modifications:

1. **Data transformation**: Convert Arrow table rows to swimlane format
   - Group rows by `id` field
   - Aggregate segments per lane
   - Convert timestamps to milliseconds

2. **Renderer component** (`SwimlaneCell`):
   - Receives `CellRendererProps` with `data`, `timeRange`, `onTimeRangeSelect`
   - Converts ISO time range to milliseconds
   - Renders swimlane visualization with drag-to-zoom support

3. **Editor component** (`SwimlaneCellEditor`):
   - SQL editor only (no additional options for V1)
   - Queries must alias columns to standard names: `id`, `name`, `begin`, `end`

4. **Metadata export** (`swimlaneMetadata`):
   - `label`: "Swimlane"
   - `icon`: "S"
   - `description`: "Horizontal lanes with time segments"
   - `defaultHeight`: 300
   - `canBlockDownstream`: true

### Step 3: Data Transformation Logic

Use the existing `timestampToMs` utility from `@/lib/arrow-utils.ts` which handles:
- Arrow JS pre-converted numbers (already milliseconds)
- Bigints with schema-aware unit conversion (SECOND/MILLISECOND/MICROSECOND/NANOSECOND)
- Date objects and string parsing
- Default: assumes nanoseconds for unknown bigints (common in Micromegas)

```typescript
import { timestampToMs } from '@/lib/arrow-utils'

interface SwimlaneLane {
  id: string
  name: string
  segments: { begin: number; end: number }[]
}

// Requires standard column names: id, name, begin, end
// Lane order is determined by first appearance in query results (use ORDER BY in SQL)
function transformDataToSwimlanes(data: Table): SwimlaneLane[] {
  // Get schema field types for timestamp conversion
  const beginField = data.schema.fields.find(f => f.name === 'begin')
  const endField = data.schema.fields.find(f => f.name === 'end')

  // Group by id, aggregate segments
  const laneMap = new Map<string, SwimlaneLane>()

  for (let i = 0; i < data.numRows; i++) {
    const id = String(data.getChild('id')?.get(i) ?? '')
    const name = String(data.getChild('name')?.get(i) ?? id)
    const begin = timestampToMs(data.getChild('begin')?.get(i), beginField?.type)
    const end = timestampToMs(data.getChild('end')?.get(i), endField?.type)

    if (!laneMap.has(id)) {
      laneMap.set(id, { id, name, segments: [] })
    }
    laneMap.get(id)!.segments.push({ begin, end })
  }

  return Array.from(laneMap.values())
}
```

### Step 4: Register in Cell Registry

**File**: `cell-registry.ts`

```typescript
import { swimlaneMetadata } from './cells/SwimlaneCell'

export const CELL_TYPE_METADATA: Record<CellType, CellTypeMetadata> = {
  table: tableMetadata,
  chart: chartMetadata,
  log: logMetadata,
  markdown: markdownMetadata,
  variable: variableMetadata,
  propertytimeline: propertyTimelineMetadata,
  swimlane: swimlaneMetadata,
}
```

### Step 5: Add Default SQL Template

**File**: `notebook-utils.ts`

```typescript
export const DEFAULT_SQL: Record<string, string> = {
  // ... existing templates ...

  swimlane: `SELECT
  stream_id as id,
  property_get("streams.properties", 'thread-name') as name,
  begin_time as begin,
  end_time as end
FROM blocks
WHERE process_id = '$process_id'
  AND array_has("streams.tags", 'cpu')
ORDER BY name, begin`,
}
```

## Component Structure

```
SwimlaneCell (renderer)
├── Header: "Swimlane" + lane count
├── Swimlane rows
│   ├── Lane name label (with truncation)
│   └── Timeline bar area
│       ├── Segment bars (positioned by %)
│       ├── Segment tooltips (on hover)
│       └── Drag selection overlay
├── Time axis (below lanes)
│   └── 5 evenly spaced tick labels (HH:MM format)
└── Empty state (when no data)

SwimlaneCellEditor
└── SQL editor (inherited from QueryCellConfig)
```

## Key Features

### Drag-to-Zoom Time Selection

Reuse the pattern from `ThreadCoverageTimeline`:
- Mouse down starts selection
- Mouse move updates selection overlay
- Mouse up triggers `onTimeRangeSelect` callback
- Minimum 5px threshold before triggering

### Segment Positioning

- Convert timestamps to percentage positions within time range
- Clamp segments to visible range (0-100%)
- Minimum segment width for visibility (0.5%)

### Tooltips

Show segment time range on hover:
```
10:23:45 - 10:24:12
```

### Time Axis

Display time axis below the lanes (reuse pattern from `PropertyTimeline`):
- 5 evenly spaced tick labels across the time range
- Format: `HH:MM` using `Intl.DateTimeFormat`
- Aligned with the timeline bar area (offset by lane label width)

```typescript
const TIME_AXIS_FORMAT = new Intl.DateTimeFormat(undefined, {
  hour: '2-digit',
  minute: '2-digit',
  hour12: false,
})

function TimeAxis({ from, to }: { from: number; to: number }) {
  const ticks = useMemo(() => {
    const count = 5
    const step = (to - from) / (count - 1)
    return Array.from({ length: count }, (_, i) => from + i * step)
  }, [from, to])

  return (
    <div className="relative h-full">
      {ticks.map((time, i) => {
        const percent = ((time - from) / (to - from)) * 100
        return (
          <span
            key={i}
            className="absolute -translate-x-1/2 text-[10px] text-theme-text-muted"
            style={{ left: `${percent}%` }}
          >
            {TIME_AXIS_FORMAT.format(time)}
          </span>
        )
      })}
    </div>
  )
}
```

### Empty State

Display message when query returns no rows:
```
No swimlane data available
```

## Visual Design

```
┌─────────────────────────────────────────────────────────────────┐
│ Swimlane                                                         │
│ 4 lanes                                                          │
├────────────────┬────────────────────────────────────────────────┤
│ main           │ |████████|     |██████████████|    |███|       │
│ worker-1       │      |██████████████████████████████████|      │
│ worker-2       │           |████████████████|                   │
│ io-thread      │ |███|  |███|  |███|  |███|  |███|  |███|       │
├────────────────┼────────────────────────────────────────────────┤
│                │ 10:00    10:15    10:30    10:45    11:00      │
└────────────────┴────────────────────────────────────────────────┘
```

## Styling

Use existing CSS classes:
- `bg-app-panel` for container background
- `border-theme-border` for borders
- `bg-chart-line` for segment bars
- `text-brand-blue` for lane labels
- `var(--chart-selection)` for drag selection overlay

## Testing

### Manual Testing
1. Create a swimlane cell with thread coverage query
2. Verify lanes display correctly
3. Test drag-to-zoom updates time range
4. Verify segment tooltips appear on hover
5. Test with empty query results

### Unit Tests (optional follow-up)
- `transformDataToSwimlanes` function
- Segment positioning calculations
- Time range clamping logic

## Migration

No migration needed - this is a new cell type. Existing notebooks are unaffected.

## Future Enhancements (out of scope)

- Column mapping UI (allow non-standard column names)
- Segment coloring based on data values
- Lane grouping/hierarchy
- Click-to-select segment (for drilling down)
- Configurable lane height
