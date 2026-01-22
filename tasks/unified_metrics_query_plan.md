# Unified Chart and Property Timeline Query Plan

## Status: PENDING

## Overview
Refactor the metrics screens so that the chart and property timeline rely on the same query. Currently there are N+1 queries: 1 for chart data, 1 for property key discovery, and N for property timeline values (one per selected property). This plan consolidates them into a single query.

## Affected Pages
- `ProcessMetricsPage.tsx` - Process metrics screen
- `PerformanceAnalysisPage.tsx` - Performance analysis screen (nearly identical pattern)

## Future Development
- `MetricsRenderer.tsx` - User-defined metrics screens (`/screen/new?type=metrics`)
  - Currently uses `XYChart` directly, not `MetricsChart`
  - No property timeline feature yet
  - Future: Add property timeline support using the same unified query pattern

## Current State

**Chart query** (`ProcessMetricsPage.tsx:22-28`):
```sql
SELECT
  date_bin(INTERVAL '$bin_interval', time) as time,
  max(value) as value
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
GROUP BY date_bin(INTERVAL '$bin_interval', time)
ORDER BY time
```

**Property keys query** (`usePropertyKeys.ts`):
```sql
SELECT DISTINCT unnest(arrow_cast(jsonb_object_keys(properties), 'List(Utf8)')) as key
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
  AND properties IS NOT NULL
ORDER BY key
```

**Property values query** (`usePropertyTimeline.ts`) - one per property:
```sql
SELECT
  date_bin(INTERVAL '$bin_interval', time) as time,
  first_value(property_get(properties, '$property_name')) as value
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
  AND properties IS NOT NULL
GROUP BY date_bin(INTERVAL '$bin_interval', time)
ORDER BY time
```

## Proposed Unified Query

```sql
SELECT
  date_bin(INTERVAL '$bin_interval', time) as time,
  max(value) as value,
  jsonb_format_json(first_value(properties) FILTER (WHERE properties IS NOT NULL)) as properties
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
GROUP BY date_bin(INTERVAL '$bin_interval', time)
ORDER BY time
```

This single query provides:
- `time`, `value` for the chart (unchanged)
- `properties` as a JSON string (e.g., `{"cpu": "high"}`) for client-side parsing

Note: `jsonb_format_json` converts JSONB binary to a JSON text string that can be parsed with `JSON.parse()`. The column can be NULL if no properties exist in that time bin.

## Implementation Steps

### 1. Create Shared Property Utility
**File:** `analytics-web-app/src/lib/property-utils.ts`

Move `aggregateIntoSegments` and `parseIntervalToMs` from `usePropertyTimeline.ts` to a shared utility.

**Bug fix:** The existing `parseIntervalToMs` function doesn't handle milliseconds, but `calculateBinInterval()` returns intervals like `'1 millisecond'`, `'50 milliseconds'`, etc. This causes incorrect segment sizing for short time ranges. Fix this when creating the shared utility.

```typescript
import { PropertySegment } from '@/types'

/**
 * Parse interval string (e.g., "50 milliseconds", "1 second") to milliseconds.
 * Fixed to handle millisecond intervals that were previously unsupported.
 */
export function parseIntervalToMs(interval: string): number {
  const match = interval.match(/^(\d+)\s*(millisecond|second|minute|hour|day)s?$/i)
  if (!match) return 60000 // default to 1 minute

  const value = parseInt(match[1], 10)
  const unit = match[2].toLowerCase()

  switch (unit) {
    case 'millisecond':
      return value
    case 'second':
      return value * 1000
    case 'minute':
      return value * 60 * 1000
    case 'hour':
      return value * 60 * 60 * 1000
    case 'day':
      return value * 24 * 60 * 60 * 1000
    default:
      return 60000
  }
}

export function aggregateIntoSegments(
  rows: { time: number; value: string }[],
  binIntervalMs: number
): PropertySegment[]
```

### 2. Update DEFAULT_SQL in Both Pages
**Files:**
- `analytics-web-app/src/routes/ProcessMetricsPage.tsx`
- `analytics-web-app/src/routes/PerformanceAnalysisPage.tsx`

Note: Steps 2-5 and 7 apply to both pages. The code is nearly identical.

Change `DEFAULT_SQL` to include the `properties` column:
```typescript
const DEFAULT_SQL = `SELECT
  date_bin(INTERVAL '$bin_interval', time) as time,
  max(value) as value,
  jsonb_format_json(first_value(properties) FILTER (WHERE properties IS NOT NULL)) as properties
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
GROUP BY date_bin(INTERVAL '$bin_interval', time)
ORDER BY time`
```

### 3. Add State and Extraction Logic in ProcessMetricsPage.tsx

Add new state:
```typescript
const [rawPropertiesData, setRawPropertiesData] = useState<Map<number, Record<string, unknown>>>(new Map())
```

Update the data extraction effect to also store properties:
```typescript
useEffect(() => {
  if (dataQuery.isComplete && !dataQuery.error) {
    const table = dataQuery.getTable()
    if (table) {
      const points: { time: number; value: number }[] = []
      const propsMap = new Map<number, Record<string, unknown>>()

      for (let i = 0; i < table.numRows; i++) {
        const row = table.get(i)
        if (row) {
          const time = timestampToMs(row.time)
          points.push({ time, value: Number(row.value) })
          // properties is a JSON string from jsonb_format_json, or null
          const propsStr = row.properties
          if (propsStr != null) {
            try {
              propsMap.set(time, JSON.parse(String(propsStr)))
            } catch {
              // Ignore parse errors
            }
          }
        }
      }

      setChartData(points)
      setRawPropertiesData(propsMap)
      setHasLoaded(true)
    }
  }
}, [dataQuery.isComplete, dataQuery.error])
```

### 4. Derive Available Property Keys

```typescript
const availablePropertyKeys = useMemo(() => {
  const keysSet = new Set<string>()
  for (const props of rawPropertiesData.values()) {
    Object.keys(props).forEach(k => keysSet.add(k))
  }
  return Array.from(keysSet).sort()
}, [rawPropertiesData])
```

### 5. Add Property Timeline Extraction Function

```typescript
const getPropertyTimeline = useCallback((propertyName: string): PropertyTimelineData => {
  const rows: { time: number; value: string }[] = []

  // Sort by time
  const sortedEntries = Array.from(rawPropertiesData.entries()).sort((a, b) => a[0] - b[0])

  for (const [time, props] of sortedEntries) {
    const value = props[propertyName]
    if (value !== undefined && value !== null) {
      rows.push({ time, value: String(value) })
    }
  }

  const binIntervalMs = parseIntervalToMs(binInterval)
  return {
    propertyName,
    segments: aggregateIntoSegments(rows, binIntervalMs),
  }
}, [rawPropertiesData, binInterval])
```

### 6. Update MetricsChart Props
**File:** `analytics-web-app/src/components/MetricsChart.tsx`

Update interface:
```typescript
interface MetricsChartProps {
  // Chart data
  data: { time: number; value: number }[]
  title: string
  unit: string
  // Property data (from unified query)
  availablePropertyKeys: string[]
  getPropertyTimeline: (key: string) => PropertyTimelineData
  // Selected properties (controlled from parent)
  selectedProperties: string[]
  onAddProperty: (key: string) => void
  onRemoveProperty: (key: string) => void
  // Callbacks
  onTimeRangeSelect?: (from: Date, to: Date) => void
  onWidthChange?: (width: number) => void
}
```

Remove:
- `processId`, `measureName`, `apiTimeRange`, `binInterval` props
- `usePropertyKeys` hook call
- `usePropertyTimeline` hook call

Replace hook calls with prop usage:
```typescript
// Instead of usePropertyKeys
const availableKeys = availablePropertyKeys

// Instead of usePropertyTimeline
const propertyTimelines = useMemo(() => {
  return selectedProperties.map(key => getPropertyTimeline(key))
}, [selectedProperties, getPropertyTimeline])
```

### 7. Update Both Pages to Pass New Props

Update MetricsChart usage in both `ProcessMetricsPage.tsx` and `PerformanceAnalysisPage.tsx`:

```typescript
<MetricsChart
  data={chartData}
  title={selectedMeasure}
  unit={selectedMeasureInfo?.unit || ''}
  availablePropertyKeys={availablePropertyKeys}
  getPropertyTimeline={getPropertyTimeline}
  selectedProperties={selectedProperties}
  onAddProperty={handleAddProperty}
  onRemoveProperty={handleRemoveProperty}
  onTimeRangeSelect={handleTimeRangeSelect}
  onWidthChange={handleChartWidthChange}
  // PerformanceAnalysisPage also passes:
  // scaleMode={scaleMode}
  // onScaleModeChange={handleScaleModeChange}
  // onAxisBoundsChange={handleAxisBoundsChange}
/>
```

### 8. Delete Unused Hooks

After completing the refactor, delete the hooks that are no longer used:

```bash
rm analytics-web-app/src/hooks/usePropertyKeys.ts
rm analytics-web-app/src/hooks/usePropertyTimeline.ts
```

Verify no other files import these hooks before deleting.

## File Changes Summary

| File | Change |
|------|--------|
| `src/lib/property-utils.ts` | New file: shared aggregation utilities (with millisecond fix) |
| `src/routes/ProcessMetricsPage.tsx` | Update SQL, add properties extraction, add getPropertyTimeline |
| `src/routes/PerformanceAnalysisPage.tsx` | Update SQL, add properties extraction, add getPropertyTimeline (same changes as ProcessMetricsPage) |
| `src/components/MetricsChart.tsx` | Remove hooks, use props for property data |
| `src/hooks/usePropertyKeys.ts` | Delete (keys now derived from unified query data) |
| `src/hooks/usePropertyTimeline.ts` | Delete (logic moved to page components) |

## Verification

1. Start dev server: `cd analytics-web-app && yarn dev`
2. Start backend: `cd rust && cargo run --bin analytics-web-srv`
3. Test ProcessMetricsPage:
   - Navigate to a process metrics page with measures that have properties
   - Verify chart displays correctly
   - Add/remove properties from the timeline and verify they display
   - Check browser Network tab - should see only 1 data query instead of N+1
   - Verify time range selection works on both chart and property timeline
4. Test PerformanceAnalysisPage:
   - Navigate to a performance analysis page
   - Repeat the same verification steps as ProcessMetricsPage
   - Verify thread coverage timeline still works correctly
5. Run `yarn lint` and `yarn type-check`
