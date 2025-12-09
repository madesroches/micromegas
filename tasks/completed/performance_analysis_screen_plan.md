# Performance Analysis Screen - Implementation Plan

## Overview

A new screen in the analytics-web-app that helps identify slow frames using metrics and correlate them with trace data before downloading a Perfetto trace.

## Requirements

1. **Metrics Graph**: User-selectable metric display with drag-to-select time range
2. **Thread Coverage Panel**: Show thread names with timeline bars indicating trace data coverage
3. **Perfetto Download**: Button to download Perfetto trace for the selected range

## Files to Create

| File | Purpose |
|------|---------|
| `analytics-web-app/src/app/performance_analysis/layout.tsx` | Next.js layout with metadata |
| `analytics-web-app/src/app/performance_analysis/page.tsx` | Main page component |
| `analytics-web-app/src/components/ThreadCoverageTimeline.tsx` | Thread timeline visualization |

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/src/app/process/page.tsx` | Add link to Performance Analysis (like Metrics, Logs, Trace) |

## SQL Queries

### Measure Discovery (reuse from process_metrics)
```sql
SELECT name, first_value(target) as target, first_value(unit) as unit
FROM view_instance('measures', '$process_id')
GROUP BY name
ORDER BY name
```

### Measures Data (reuse from process_metrics)
```sql
SELECT date_bin(INTERVAL '$bin_interval', time) as time, max(value) as value
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
GROUP BY date_bin(INTERVAL '$bin_interval', time)
ORDER BY time
```

### Thread Coverage (NEW)
```sql
SELECT
  stream_id,
  property_get("streams.properties", 'thread-name') as thread_name,
  begin_time,
  end_time
FROM blocks
WHERE process_id = '$process_id'
  AND array_has("streams.tags", 'cpu')
ORDER BY thread_name, begin_time
```

## Component Structure

```
PerformanceAnalysisPage
├── AuthGuard
│   └── Suspense
│       └── PerformanceAnalysisContent
│           └── PageLayout (with QueryEditor in rightPanel)
│               ├── Header: Back link, title, CopyableProcessId
│               ├── Controls: Measure dropdown, selection range display, Download button
│               ├── TimeSeriesChart (with onTimeRangeSelect for drag selection)
│               └── ThreadCoverageTimeline (aligned time axis, coverage bars)
```

## ThreadCoverageTimeline Component

Visual representation:
```
┌─────────────────────────────────────────────────────────────────┐
│ Thread Coverage                                                  │
├────────────────┬────────────────────────────────────────────────┤
│ main           │ |████████|     |██████████████|    |███|       │
│ worker-1       │      |██████████████████████████████████|      │
│ worker-2       │           |████████████████|                   │
│ io-thread      │ |███|  |███|  |███|  |███|  |███|  |███|       │
└────────────────┴────────────────────────────────────────────────┘
```

Props:
- `threads`: Array of { stream_id, thread_name, segments: { begin, end }[] }
- `timeRange`: { from, to } - aligned with chart
- `selectedRange`: Optional highlight overlay

## State Management

URL params for persistence:
- `process_id` - Required
- `measure` - Selected measure name
- `sel_from` / `sel_to` - Selected range for Perfetto download

## Implementation Steps

### Step 1: Page Structure
- Create layout.tsx and page.tsx following process_metrics pattern
- AuthGuard, Suspense, PageLayout structure

### Step 2: Metrics Chart
- Copy measure discovery from process_metrics
- Implement measure selector dropdown
- Integrate TimeSeriesChart with onTimeRangeSelect

### Step 3: Thread Coverage Timeline
- Create ThreadCoverageTimeline component
- Query thread coverage data
- Render timeline bars aligned with chart

### Step 4: Perfetto Download
- Add Download button
- Wire up generateTrace API with selected range
- Progress modal during generation

### Step 5: Integration
- Add link from process details page (alongside Metrics, Logs, Trace buttons)
- Handle empty states and errors

## Reference Files

- `analytics-web-app/src/app/process_metrics/page.tsx` - Page pattern, measure discovery, chart
- `analytics-web-app/src/app/process_trace/page.tsx` - Perfetto generation with progress
- `analytics-web-app/src/components/TimeSeriesChart.tsx` - Chart with selection
- `analytics-web-app/src/lib/api.ts` - executeSqlQuery, generateTrace

## Backend Changes

None required - existing APIs support all functionality.
