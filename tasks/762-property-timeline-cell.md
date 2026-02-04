# Implementation Plan: Property Timeline Cell Type (#762)

## Overview

Add a new notebook cell type that displays property timeline visualizations - horizontal bars showing how categorical property values change over time.

## Architecture

The notebook uses a plugin-based cell architecture:
- **Type definitions** in `notebook-types.ts` - discriminated union of cell configs
- **Metadata registry** in `cell-registry.ts` - renderer, editor, execute function per cell type
- **Cell components** in `cells/` folder - one file per cell type

## Implementation Steps

### 1. Define Config Type

**File:** `analytics-web-app/src/lib/screen-renderers/notebook-types.ts`

Add to the CellType union:
```typescript
export type CellType = 'table' | 'chart' | 'log' | 'markdown' | 'variable' | 'propertytimeline'
```

Add config interface:
```typescript
export interface PropertyTimelineCellConfig extends CellConfigBase {
  type: 'propertytimeline'
  sql: string
  options?: {
    selectedKeys?: string[]      // Which property keys to display
  }
}
```

Add to CellConfig union:
```typescript
export type CellConfig = TableCellConfig | ChartCellConfig | LogCellConfig | MarkdownCellConfig | VariableCellConfig | PropertyTimelineCellConfig
```

### 2. Create PropertyTimelineCell Component

**File:** `analytics-web-app/src/lib/screen-renderers/cells/PropertyTimelineCell.tsx`

#### Expected SQL Query Schema

The cell expects a query returning rows with:
- `time: timestamp` - the timestamp for this data point
- `properties: string` - JSON object with property key/value pairs

Example query:
```sql
SELECT
  time,
  jsonb_format_json(properties) as properties
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
ORDER BY time
```

The cell parses the JSON properties and aggregates adjacent rows with the same value into segments. Each segment runs from its timestamp until the next row's timestamp (no explicit bin interval needed).

#### Renderer Component

```typescript
interface PropertyTimelineCellRendererProps extends CellRendererProps {
  options?: {
    selectedKeys?: string[]
  }
}
```

The renderer:
1. Extracts data from Arrow table
2. Transforms to `PropertyTimelineData[]` format
3. Derives available keys from data
4. Manages selected keys state (from options or default to first N keys)
5. Renders existing `PropertyTimeline` component

#### Editor Component

The editor provides:
- SQL query input (reuse existing SQL editor pattern)
- Property key multi-select (populated after query runs)

#### Metadata Export

```typescript
export const propertyTimelineMetadata: CellTypeMetadata = {
  renderer: PropertyTimelineCellRenderer,
  EditorComponent: PropertyTimelineCellEditor,
  label: 'Property Timeline',
  icon: '▬',
  description: 'Display property values over time as horizontal segments',
  showTypeBadge: true,
  defaultHeight: 200,
  canBlockDownstream: false,
  createDefaultConfig: () => ({
    type: 'propertytimeline' as const,
    sql: '',
    options: { selectedKeys: [] }
  }),
  execute: async (config, context) => {
    const sql = substituteMacros(config.sql, context.variables, context.timeRange)
    const data = await context.runQuery(sql)
    return { data }
  },
  getRendererProps: (config, state) => ({
    sql: config.sql,
    options: config.options,
    data: state.data
  })
}
```

### 3. Register Cell Type

**File:** `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`

```typescript
import { propertyTimelineMetadata } from './cells/PropertyTimelineCell'

export const CELL_TYPE_METADATA: Record<CellType, CellTypeMetadata> = {
  table: tableMetadata,
  chart: chartMetadata,
  log: logMetadata,
  markdown: markdownMetadata,
  variable: variableMetadata,
  propertytimeline: propertyTimelineMetadata,
}
```

### 4. Data Transformation

**File:** `analytics-web-app/src/lib/screen-renderers/cells/PropertyTimelineCell.tsx`

Leverage existing utilities from `property-utils.ts`:

```typescript
import { extractPropertiesFromRows } from '@/lib/property-utils'

function transformToPropertyTimelines(
  table: Table,
  selectedKeys: string[]
): { timelines: PropertyTimelineData[], availableKeys: string[] } {
  // 1. Extract rows as { time, properties } from Arrow table
  const rows = extractRowsFromTable(table)

  // 2. Parse JSON properties and collect available keys
  const { availableKeys, rawData } = extractPropertiesFromRows(rows)

  // 3. For each selected key, aggregate into segments
  const timelines = selectedKeys.map(key => ({
    propertyName: key,
    segments: aggregateIntoSegmentsInferred(rawData, key)
  }))

  return { timelines, availableKeys }
}
```

**New aggregation function** (or modify existing):
```typescript
function aggregateIntoSegmentsInferred(
  rawData: Map<number, Record<string, unknown>>,
  propertyName: string
): PropertySegment[] {
  // Sort timestamps, iterate through
  // Each segment ends at the next timestamp
  // Last segment: extend by same delta as previous interval
}
```

### 5. Reuse Existing PropertyTimeline Component

**File:** `analytics-web-app/src/components/PropertyTimeline.tsx`

The existing component handles:
- Rendering horizontal segment bars
- Tooltips with property name, value, time range
- Add/remove property selection
- Time range display alignment

Props to provide from cell:
```typescript
<PropertyTimeline
  properties={timelines}
  availableKeys={availableKeys}
  selectedKeys={selectedKeys}
  timeRange={{ from: beginMs, to: endMs }}
  onAddProperty={(key) => updateOptions({ selectedKeys: [...selectedKeys, key] })}
  onRemoveProperty={(key) => updateOptions({ selectedKeys: selectedKeys.filter(k => k !== key) })}
  isLoading={status === 'loading'}
/>
```

Note: `axisBounds` and `onTimeRangeSelect` are for chart synchronization - defer to #765.

## File Changes Summary

| File | Change |
|------|--------|
| `notebook-types.ts` | Add `propertytimeline` to CellType, add PropertyTimelineCellConfig |
| `cell-registry.ts` | Import and register propertyTimelineMetadata |
| `cells/PropertyTimelineCell.tsx` | New file - renderer, editor, metadata |
| `property-utils.ts` | Add `aggregateIntoSegmentsInferred()` that derives segment boundaries from timestamp deltas |

## Testing

1. Create notebook with property timeline cell
2. Configure SQL query returning property segments
3. Verify segments render correctly
4. Verify add/remove property key selection works
5. Verify cell re-executes on variable changes

## Future Considerations (Out of Scope)

- Time range selection synchronization (#765)
- Axis alignment with chart cells
- Custom color schemes per property value
