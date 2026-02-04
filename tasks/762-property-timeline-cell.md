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

Add `'propertytimeline'` to the CellType union:
```typescript
export type CellType = 'table' | 'chart' | 'log' | 'markdown' | 'variable' | 'propertytimeline'
```

Add `'propertytimeline'` to the QueryCellConfig type union:
```typescript
export interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log' | 'propertytimeline'
  sql: string
  options?: Record<string, unknown>
}
```

The `options` field remains untyped (`Record<string, unknown>`) per the open/closed principle. The PropertyTimeline cell will use `options.selectedKeys` internally, interpreted as `string[]`.

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

The renderer receives standard `CellRendererProps` with `options?: Record<string, unknown>`. It interprets `options.selectedKeys` as `string[]` internally.

The renderer:
1. Extracts data from Arrow table
2. Transforms to `PropertyTimelineData[]` format
3. Derives available keys from data
4. Reads selected keys from `options.selectedKeys` (empty by default)
5. Renders existing `PropertyTimeline` component with add/remove callbacks

Property selection UX (same as PerformanceAnalysisPage):
- Starts with no properties selected
- User clicks "Add Property" dropdown to select from available keys
- User clicks remove button to deselect a property
- Selection persisted in `options.selectedKeys`

#### Editor Component

The editor provides:
- SQL query input (reuse existing SQL editor pattern)
- No property selection in editor - handled by renderer's PropertyTimeline component

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
  canBlockDownstream: true,
  createDefaultConfig: () => ({
    type: 'propertytimeline' as const,
    sql: DEFAULT_SQL.propertytimeline,
    options: {}
  }),
  execute: async (config, context) => {
    const sql = substituteMacros(config.sql, context.variables, context.timeRange)
    const data = await context.runQuery(sql)
    return { data }
  },
  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
    options: (config as QueryCellConfig).options,
  })
}
```

### 3. Add Default SQL

**File:** `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`

Add to `DEFAULT_SQL`:
```typescript
export const DEFAULT_SQL: Record<string, string> = {
  // ... existing entries ...
  propertytimeline: `SELECT time, jsonb_format_json(properties) as properties
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
ORDER BY time`,
}
```

### 4. Register Cell Type

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

### 5. Refactor property-utils.ts

**File:** `analytics-web-app/src/lib/property-utils.ts`

#### Refactor `extractPropertiesFromRows()` - Add Error Reporting

The current implementation silently ignores JSON parse errors. Change to report errors clearly:

```typescript
export interface ExtractedPropertyData {
  availableKeys: string[]
  rawData: Map<number, Record<string, unknown>>
  errors: string[]  // Add error reporting
}

export function extractPropertiesFromRows(
  rows: { time: number; properties: string | null }[]
): ExtractedPropertyData {
  const rawData = new Map<number, Record<string, unknown>>()
  const keysSet = new Set<string>()
  const errors: string[] = []

  for (const row of rows) {
    if (row.properties != null) {
      try {
        const props = JSON.parse(row.properties)
        rawData.set(row.time, props)
        Object.keys(props).forEach(k => keysSet.add(k))
      } catch (e) {
        errors.push(`Invalid JSON at time ${row.time}: ${e instanceof Error ? e.message : String(e)}`)
      }
    }
  }

  return {
    availableKeys: Array.from(keysSet).sort(),
    rawData,
    errors,
  }
}
```

Call sites should check `errors.length > 0` and display warnings to the user.

#### Refactor `aggregateIntoSegments()`

The current implementation takes a `binIntervalMs` parameter, but this is redundant - segment boundaries should be derived from the data itself.

Remove the `binIntervalMs` parameter. Add optional `timeRange` parameter for first/last segment boundaries:

```typescript
export function aggregateIntoSegments(
  rows: { time: number; value: string }[],
  timeRange?: { begin: number; end: number }
): PropertySegment[] {
  if (rows.length === 0) return []

  const segments: PropertySegment[] = []
  let currentSegment: PropertySegment | null = null

  for (let i = 0; i < rows.length; i++) {
    const row = rows[i]
    const nextTime = rows[i + 1]?.time

    if (!currentSegment) {
      currentSegment = {
        value: row.value,
        begin: timeRange?.begin ?? row.time,
        end: nextTime ?? timeRange?.end ?? row.time,
      }
    } else if (currentSegment.value === row.value) {
      // Extend current segment
      currentSegment.end = nextTime ?? timeRange?.end ?? row.time
    } else {
      // Close current segment at this row's time, start new one
      currentSegment.end = row.time
      segments.push(currentSegment)
      currentSegment = {
        value: row.value,
        begin: row.time,
        end: nextTime ?? timeRange?.end ?? row.time,
      }
    }
  }

  if (currentSegment) {
    segments.push(currentSegment)
  }

  return segments
}
```

#### Refactor `createPropertyTimelineGetter()`

Remove the `binInterval` parameter, add optional `timeRange`:

```typescript
export function createPropertyTimelineGetter(
  rawData: Map<number, Record<string, unknown>>,
  timeRange?: { begin: number; end: number }
): (propertyName: string) => PropertyTimelineData {
  return (propertyName: string): PropertyTimelineData => {
    const rows: { time: number; value: string }[] = []
    const sortedEntries = Array.from(rawData.entries()).sort((a, b) => a[0] - b[0])

    for (const [time, props] of sortedEntries) {
      const value = props[propertyName]
      if (value !== undefined && value !== null) {
        rows.push({ time, value: String(value) })
      }
    }

    return {
      propertyName,
      segments: aggregateIntoSegments(rows, timeRange),
    }
  }
}
```

#### Remove `parseIntervalToMs()`

This function is no longer needed in property-utils.ts.

#### Update call sites

- `useMetricsData.ts:129-132` - Remove `binIntervalMs`, pass time range: `aggregateIntoSegments(rows, timeRange)`
- `ProcessMetricsPage.tsx:169` - Replace `binInterval` with time range in `createPropertyTimelineGetter(rawData, timeRange)`
- `PerformanceAnalysisPage.tsx:201` - Replace `binInterval` with time range in `createPropertyTimelineGetter(rawData, timeRange)`

Note: All these call sites already have access to the query time range (in ms), so passing it is straightforward.

### 6. Data Transformation in Cell

**File:** `analytics-web-app/src/lib/screen-renderers/cells/PropertyTimelineCell.tsx`

Leverage the refactored utilities from `property-utils.ts`:

```typescript
import { extractPropertiesFromRows, createPropertyTimelineGetter } from '@/lib/property-utils'
import { timestampToMs } from '@/lib/arrow-utils'

/** Extract time and properties columns from Arrow table */
function extractRowsFromTable(table: Table): { time: number; properties: string | null }[] {
  const rows: { time: number; properties: string | null }[] = []
  const timeCol = table.getChild('time')
  const propsCol = table.getChild('properties')

  if (!timeCol) return rows

  for (let i = 0; i < table.numRows; i++) {
    const time = timestampToMs(timeCol.get(i))
    const properties = propsCol?.get(i) ?? null
    rows.push({ time, properties })
  }
  return rows
}

function transformToPropertyTimelines(
  table: Table,
  selectedKeys: string[],
  timeRange: { begin: number; end: number }
): { timelines: PropertyTimelineData[], availableKeys: string[], errors: string[] } {
  // 1. Extract rows as { time, properties } from Arrow table
  const rows = extractRowsFromTable(table)

  // 2. Parse JSON properties and collect available keys
  const { availableKeys, rawData, errors } = extractPropertiesFromRows(rows)

  // 3. Create getter and build timelines for selected keys
  const getTimeline = createPropertyTimelineGetter(rawData, timeRange)
  const timelines = selectedKeys.map(key => getTimeline(key))

  return { timelines, availableKeys, errors }
}
```

Time range conversion in the renderer (ISO strings to ms):
```typescript
const beginMs = new Date(timeRange.begin).getTime()
const endMs = new Date(timeRange.end).getTime()
```

### 7. Reuse Existing PropertyTimeline Component

**File:** `analytics-web-app/src/components/PropertyTimeline.tsx`

The existing component handles:
- Rendering horizontal segment bars
- Tooltips with property name, value, time range
- Add/remove property selection
- Time range display alignment

Props to provide from cell:
```typescript
const selectedKeys = (options?.selectedKeys as string[]) ?? []

<PropertyTimeline
  properties={timelines}
  availableKeys={availableKeys}
  selectedKeys={selectedKeys}
  timeRange={{ from: beginMs, to: endMs }}
  onAddProperty={(key) => onOptionsChange({ ...options, selectedKeys: [...selectedKeys, key] })}
  onRemoveProperty={(key) => onOptionsChange({ ...options, selectedKeys: selectedKeys.filter(k => k !== key) })}
  isLoading={status === 'loading'}
/>
```

Note: `axisBounds` and `onTimeRangeSelect` are for chart synchronization - defer to #765.

## File Changes Summary

| File | Change |
|------|--------|
| `notebook-types.ts` | Add `'propertytimeline'` to CellType and QueryCellConfig type unions |
| `notebook-utils.ts` | Add `DEFAULT_SQL.propertytimeline` |
| `cell-registry.ts` | Import and register propertyTimelineMetadata |
| `cells/PropertyTimelineCell.tsx` | New file - renderer, editor, metadata |
| `property-utils.ts` | Add error reporting to `extractPropertiesFromRows()`, refactor `aggregateIntoSegments()` to derive boundaries from data, remove `binInterval` params, remove `parseIntervalToMs()` |
| `useMetricsData.ts` | Update `aggregateIntoSegments()` call (remove interval arg) |
| `ProcessMetricsPage.tsx` | Update `createPropertyTimelineGetter()` call (remove interval arg) |
| `PerformanceAnalysisPage.tsx` | Update `createPropertyTimelineGetter()` call (remove interval arg) |

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
