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
WHERE name = 'cpu_usage'
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
5. Displays parse errors (if any) as a warning banner
6. Renders existing `PropertyTimeline` component with add/remove callbacks

#### Error Display

When `extractPropertiesFromRows()` returns errors (invalid JSON in properties column), display a warning banner above the PropertyTimeline component:

```tsx
{errors.length > 0 && (
  <div className="mb-2 px-3 py-2 bg-amber-500/10 border border-amber-500/30 rounded text-amber-400 text-xs">
    <span className="font-medium">Warning:</span> {errors.length} row(s) had invalid JSON properties and were skipped.
    <details className="mt-1">
      <summary className="cursor-pointer hover:text-amber-300">Show details</summary>
      <ul className="mt-1 ml-4 list-disc text-amber-400/80">
        {errors.slice(0, 5).map((err, i) => <li key={i}>{err}</li>)}
        {errors.length > 5 && <li>...and {errors.length - 5} more</li>}
      </ul>
    </details>
  </div>
)}
```

The warning is non-blocking - valid rows still render. The collapsible details show the first 5 errors to help debugging without overwhelming the UI.

Property selection UX (same as PerformanceAnalysisPage):
- Starts with no properties selected
- User clicks "Add Property" dropdown to select from available keys
- User clicks remove button to deselect a property
- Selection persisted in `options.selectedKeys`

#### Editor Component

The editor provides:
- SQL query input (reuse existing SQL editor pattern)
- SQL macro validation (same as ChartCellEditor)
- AvailableVariablesPanel showing variables that can be referenced
- No property selection in editor - handled by renderer's PropertyTimeline component

```typescript
function PropertyTimelineCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const ptConfig = config as QueryCellConfig

  // Validate macro references in SQL
  const validationErrors = useMemo(() => {
    return validateMacros(ptConfig.sql, variables).errors
  }, [ptConfig.sql, variables])

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={ptConfig.sql}
          onChange={(sql) => onChange({ ...ptConfig, sql })}
          language="sql"
          placeholder="SELECT time, properties FROM ..."
          minHeight="150px"
        />
      </div>
      {validationErrors.length > 0 && (
        <div className="text-red-400 text-sm space-y-1">
          {validationErrors.map((err, i) => (
            <div key={i}>⚠ {err}</div>
          ))}
        </div>
      )}
      <AvailableVariablesPanel variables={variables} timeRange={timeRange} />
    </>
  )
}
```

#### Metadata Export

```typescript
export const propertyTimelineMetadata: CellTypeMetadata = {
  renderer: PropertyTimelineCellRenderer,
  EditorComponent: PropertyTimelineCellEditor,
  label: 'Property Timeline',
  icon: 'P',
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
WHERE name = 'cpu_usage'
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
      // First segment starts at actual data point (not timeRange.begin)
      // to align with chart rendering
      currentSegment = {
        value: row.value,
        begin: row.time,
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

### 8. Add Time Axis to PropertyTimeline Component

**File:** `analytics-web-app/src/components/PropertyTimeline.tsx`

Add an optional time axis at the bottom of the component showing tick marks and labels.

Add new prop:
```typescript
interface PropertyTimelineProps {
  // ... existing props ...
  showTimeAxis?: boolean  // Default false for backwards compatibility
}
```

Render time axis when `showTimeAxis` is true:
```tsx
{showTimeAxis && (
  <div className="flex items-center h-6 text-[10px] text-theme-text-muted">
    {/* Spacer matching label column width */}
    <div style={{ width: leftOffset }} />
    {/* Axis area */}
    <div className="relative" style={{ width: plotWidth ?? '100%' }}>
      <TimeAxis from={timeRange.from} to={timeRange.to} />
    </div>
  </div>
)}
```

Create a simple `TimeAxis` component that renders 3-5 evenly spaced tick marks with time labels:
```tsx
function TimeAxis({ from, to }: { from: number; to: number }) {
  const ticks = useMemo(() => {
    const count = 5
    const step = (to - from) / (count - 1)
    return Array.from({ length: count }, (_, i) => from + i * step)
  }, [from, to])

  const formatTick = (time: number) => {
    const d = new Date(time)
    return d.toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', hour12: false })
  }

  return (
    <div className="relative h-full">
      {ticks.map((time, i) => {
        const percent = ((time - from) / (to - from)) * 100
        return (
          <span
            key={time}
            className="absolute -translate-x-1/2"
            style={{ left: `${percent}%` }}
          >
            {formatTick(time)}
          </span>
        )
      })}
    </div>
  )
}
```

Pass `showTimeAxis={true}` from PropertyTimelineCell renderer.

## File Changes Summary

| File | Change |
|------|--------|
| `notebook-types.ts` | Add `'propertytimeline'` to CellType and QueryCellConfig type unions |
| `notebook-utils.ts` | Add `DEFAULT_SQL.propertytimeline` |
| `cell-registry.ts` | Import and register propertyTimelineMetadata |
| `cells/PropertyTimelineCell.tsx` | New file - renderer, editor, metadata |
| `components/PropertyTimeline.tsx` | Add `showTimeAxis` prop and `TimeAxis` sub-component |
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
