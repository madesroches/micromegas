# Unified Chart and Property Timeline Query Plan

## Status: IMPLEMENTED

## Related Plans
- [Dictionary Preservation](./dictionary_preservation_plan.md) - Bandwidth optimization for dictionary-encoded columns (recommended to implement first)

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

## Architecture

This refactor maintains MVC separation:

| Layer | Component | Responsibility |
|-------|-----------|----------------|
| **Model** | `useMetricsData` hook | Unified query execution, data extraction, property timeline derivation |
| **View** | `MetricsChart` | Pure presentation, receives all data via props |
| **Controller** | Page components | Wire Model to View, handle user interactions, URL state |

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

### 2. Create Unified Data Hook (Model Layer)
**File:** `analytics-web-app/src/hooks/useMetricsData.ts`

This hook serves as the Model layer, encapsulating the unified query and all data transformations. Both page components will use this hook, eliminating code duplication.

```typescript
import { useState, useEffect, useMemo, useCallback, useRef } from 'react'
import { useStreamQuery } from './useStreamQuery'
import { timestampToMs } from '@/lib/arrow-utils'
import { parseIntervalToMs, aggregateIntoSegments } from '@/lib/property-utils'
import { PropertyTimelineData } from '@/types'

const METRICS_SQL = `SELECT
  date_bin(INTERVAL '$bin_interval', time) as time,
  max(value) as value,
  jsonb_format_json(first_value(properties) FILTER (WHERE properties IS NOT NULL)) as properties
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
GROUP BY date_bin(INTERVAL '$bin_interval', time)
ORDER BY time`

interface UseMetricsDataParams {
  processId: string | null
  measureName: string | null
  binInterval: string
  apiTimeRange: { begin: string; end: string }
  enabled?: boolean
}

interface UseMetricsDataReturn {
  chartData: { time: number; value: number }[]
  availablePropertyKeys: string[]
  getPropertyTimeline: (key: string) => PropertyTimelineData
  isLoading: boolean
  isComplete: boolean
  error: string | null
  execute: () => void
}

export function useMetricsData({
  processId,
  measureName,
  binInterval,
  apiTimeRange,
  enabled = true,
}: UseMetricsDataParams): UseMetricsDataReturn {
  const query = useStreamQuery()
  const abortControllerRef = useRef<AbortController | null>(null)

  const [chartData, setChartData] = useState<{ time: number; value: number }[]>([])
  const [rawPropertiesData, setRawPropertiesData] = useState<Map<number, Record<string, unknown>>>(new Map())

  // Execute the unified query
  const execute = useCallback(() => {
    if (!processId || !measureName || !enabled) return

    // Cancel any in-flight query before starting a new one
    if (abortControllerRef.current) {
      abortControllerRef.current.abort()
    }
    abortControllerRef.current = new AbortController()

    // Clear previous data to avoid stale state
    setChartData([])
    setRawPropertiesData(new Map())

    query.execute({
      sql: METRICS_SQL,
      params: {
        process_id: processId,
        measure_name: measureName,
        bin_interval: binInterval,
      },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
      signal: abortControllerRef.current.signal,
    })
  }, [processId, measureName, binInterval, apiTimeRange, enabled, query])

  // Cleanup: cancel query on unmount
  useEffect(() => {
    return () => {
      if (abortControllerRef.current) {
        abortControllerRef.current.abort()
      }
    }
  }, [])

  // Extract data when query completes
  useEffect(() => {
    // Ignore results if query was aborted
    if (query.isComplete && !query.error && !abortControllerRef.current?.signal.aborted) {
      const table = query.getTable()
      if (table) {
        const points: { time: number; value: number }[] = []
        const propsMap = new Map<number, Record<string, unknown>>()

        for (let i = 0; i < table.numRows; i++) {
          const row = table.get(i)
          if (row) {
            const time = timestampToMs(row.time)
            points.push({ time, value: Number(row.value) })

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
      }
    }
  }, [query.isComplete, query.error])

  // Derive available property keys from the data
  const availablePropertyKeys = useMemo(() => {
    const keysSet = new Set<string>()
    for (const props of rawPropertiesData.values()) {
      Object.keys(props).forEach(k => keysSet.add(k))
    }
    return Array.from(keysSet).sort()
  }, [rawPropertiesData])

  // Function to get property timeline for a specific key
  const getPropertyTimeline = useCallback((propertyName: string): PropertyTimelineData => {
    const rows: { time: number; value: string }[] = []

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

  return {
    chartData,
    availablePropertyKeys,
    getPropertyTimeline,
    isLoading: query.isStreaming,
    isComplete: query.isComplete,
    error: query.error?.message ?? null,
    execute,
  }
}
```

### 3. Update Page Components to Use the Hook
**Files:**
- `analytics-web-app/src/routes/ProcessMetricsPage.tsx`
- `analytics-web-app/src/routes/PerformanceAnalysisPage.tsx`

Both pages will use the new `useMetricsData` hook instead of managing data extraction directly. This keeps the pages thin (Controller layer) while the hook handles data logic (Model layer).

**Remove from both pages:**
- `DEFAULT_SQL` constant (now in the hook)
- `dataQuery` hook usage for metrics data
- `chartData` state and its extraction effect
- Direct `useStreamQuery` call for metrics (keep it for discovery query)

**Add to both pages:**
```typescript
import { useMetricsData } from '@/hooks/useMetricsData'

// Inside component:
const metricsData = useMetricsData({
  processId,
  measureName: selectedMeasure,
  binInterval,
  apiTimeRange,
  enabled: !!processId && !!selectedMeasure,
})

// Use metricsData.chartData instead of chartData state
// Use metricsData.availablePropertyKeys for property keys
// Use metricsData.getPropertyTimeline for property timeline data
// Use metricsData.isLoading, metricsData.error for loading/error states
```

**Trigger data loading** when measure is selected or time range changes:
```typescript
useEffect(() => {
  if (discoveryDone && selectedMeasure && processId) {
    metricsData.execute()
  }
  // Note: metricsData.execute is stable (useCallback) and depends on apiTimeRange/binInterval internally
}, [discoveryDone, selectedMeasure, processId, metricsData.execute])
```

**Update references throughout the component:**
- Replace `chartData` with `metricsData.chartData`
- Replace `dataQuery.isStreaming` with `metricsData.isLoading`
- Replace `dataQuery.error` with `metricsData.error`
- Replace `hasLoaded` checks with `metricsData.isComplete`

### 4. Update MetricsChart Props (View Layer)
**File:** `analytics-web-app/src/components/MetricsChart.tsx`

MetricsChart becomes a pure View component that receives all data via props.

Update interface:
```typescript
interface MetricsChartProps {
  // Chart data
  data: { time: number; value: number }[]
  title: string
  unit: string
  // Property data (from unified query via Model layer)
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

Remove these props (no longer needed since Model layer provides pre-computed data):
- `processId`, `measureName`, `apiTimeRange` - were only used by the hooks to fetch data
- `binInterval` - segmentation now happens in Model's `getPropertyTimeline()` which calls `parseIntervalToMs(binInterval)` internally

Remove these hook calls (replaced by props from Model layer):
- `usePropertyKeys` - replaced by `availablePropertyKeys` prop
- `usePropertyTimeline` - replaced by `getPropertyTimeline` prop

Replace hook calls with prop usage:
```typescript
// Instead of usePropertyKeys
const availableKeys = availablePropertyKeys

// Instead of usePropertyTimeline
const propertyTimelines = useMemo(() => {
  return selectedProperties.map(key => getPropertyTimeline(key))
}, [selectedProperties, getPropertyTimeline])
```

### 5. Update Both Pages to Pass New Props

Update MetricsChart usage in both `ProcessMetricsPage.tsx` and `PerformanceAnalysisPage.tsx`:

```typescript
<MetricsChart
  data={metricsData.chartData}
  title={selectedMeasure}
  unit={selectedMeasureInfo?.unit || ''}
  availablePropertyKeys={metricsData.availablePropertyKeys}
  getPropertyTimeline={metricsData.getPropertyTimeline}
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

### 6. Delete Unused Hooks

After completing the refactor, delete the hooks that are no longer used:

```bash
rm analytics-web-app/src/hooks/usePropertyKeys.ts
rm analytics-web-app/src/hooks/usePropertyTimeline.ts
```

Verify no other files import these hooks before deleting.

## File Changes Summary

| File | Layer | Change |
|------|-------|--------|
| `analytics-web-app/src/lib/property-utils.ts` | Utility | New file: shared aggregation utilities (with millisecond fix) |
| `analytics-web-app/src/hooks/useMetricsData.ts` | Model | New file: unified data hook encapsulating query + extraction + transformation |
| `analytics-web-app/src/routes/ProcessMetricsPage.tsx` | Controller | Use `useMetricsData` hook, remove embedded data logic |
| `analytics-web-app/src/routes/PerformanceAnalysisPage.tsx` | Controller | Use `useMetricsData` hook, remove embedded data logic |
| `analytics-web-app/src/components/MetricsChart.tsx` | View | Remove hooks, receive all data via props (pure presentation) |
| `analytics-web-app/src/hooks/usePropertyKeys.ts` | - | Delete (replaced by `useMetricsData.availablePropertyKeys`) |
| `analytics-web-app/src/hooks/usePropertyTimeline.ts` | - | Delete (replaced by `useMetricsData.getPropertyTimeline`) |

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
