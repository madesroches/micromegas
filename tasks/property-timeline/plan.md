# Property Timeline Feature Implementation Plan

## Mockup
See [mockup-v1.html](mockup-v1.html) for the visual design.

## Overview
Create a reusable MetricsChart component combining a time series chart with a property timeline. The property timeline shows how measure properties change over time. Users can select which properties to display via a dropdown.

## SQL Queries

**1. Discover available property keys:**
```sql
SELECT DISTINCT unnest(arrow_cast(jsonb_object_keys(properties), 'List(Utf8)')) as key
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
  AND properties IS NOT NULL
ORDER BY key
```

**2. Get property values over time (binned):**
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

## Implementation Steps

### 1. Add Types
**File:** `analytics-web-app/src/types/index.ts`

```typescript
export interface PropertySegment {
  value: string;
  begin: number;  // ms timestamp
  end: number;    // ms timestamp
}

export interface PropertyTimelineData {
  propertyName: string;
  segments: PropertySegment[];
}
```

### 2. Create PropertyTimeline Component
**File:** `analytics-web-app/src/components/PropertyTimeline.tsx`

Based on ThreadCoverageTimeline pattern:
- Props: `properties`, `availableKeys`, `timeRange`, `axisBounds`, `onTimeRangeSelect`, `onAddProperty`, `onRemoveProperty`
- Empty state when no properties selected
- "Add property" dropdown in header
- Swimlane rows for each selected property
- Segments colored with `brand-blue`
- Tooltip on hover showing value and time range
- Remove button (X) on row hover

### 3. Create MetricsChart Component
**File:** `analytics-web-app/src/components/MetricsChart.tsx`

Combines TimeSeriesChart + PropertyTimeline:
- Props: `processId`, `measureName`, `timeRange`, `onTimeRangeSelect`
- Internal state: `selectedProperties`, `axisBounds`
- Fetches property keys and values
- Renders TimeSeriesChart + PropertyTimeline aligned

### 4. Create usePropertyKeys Hook
**File:** `analytics-web-app/src/hooks/usePropertyKeys.ts`

- Input: `processId`, `measureName`, `timeRange`
- Output: `string[]` of property key names

### 5. Create usePropertyTimeline Hook
**File:** `analytics-web-app/src/hooks/usePropertyTimeline.ts`

- Input: `processId`, `measureName`, `propertyNames`, `timeRange`
- Output: `PropertyTimelineData[]`
- Aggregates consecutive rows with same value into segments

### 6. Update Pages
- `analytics-web-app/src/routes/ProcessMetricsPage.tsx`
- `analytics-web-app/src/routes/PerformanceAnalysisPage.tsx`

Replace TimeSeriesChart with MetricsChart.

## File Changes Summary

| File | Change |
|------|--------|
| `src/types/index.ts` | Add PropertySegment, PropertyTimelineData |
| `src/components/PropertyTimeline.tsx` | New component |
| `src/components/MetricsChart.tsx` | New composite component |
| `src/hooks/usePropertyKeys.ts` | New hook |
| `src/hooks/usePropertyTimeline.ts` | New hook |
| `src/routes/ProcessMetricsPage.tsx` | Use MetricsChart |
| `src/routes/PerformanceAnalysisPage.tsx` | Use MetricsChart |

## Component Structure

```
MetricsChart
├── TimeSeriesChart (existing)
│   └── reports axisBounds
└── PropertyTimeline
    ├── Header ("Properties" + "Add property" dropdown)
    ├── Property rows (label + segments)
    ├── Time axis
    └── Empty state
```
