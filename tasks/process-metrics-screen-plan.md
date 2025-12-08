# Process Metrics Screen Plan

## Overview

Add a metrics screen to the analytics web app, accessible from the process page. The user selects a specific measure by name from a dropdown, then views the time-series data as a chart. Like the log screen, custom SQL queries are supported.

## Data Source

The metrics data uses the `measures` view with the following schema:

| Column | Type | Description |
|--------|------|-------------|
| `time` | Timestamp | When the metric was recorded |
| `target` | String | Module/context path (e.g., `myapp::performance`) |
| `name` | String | Metric name (e.g., `frame_time`, `memory_usage`) |
| `unit` | String | Unit of measurement (e.g., `ms`, `bytes`) |
| `value` | Float64 | The numeric value |
| `properties` | JSONB | Additional per-metric properties |
| `process_id` | String | Process identifier |

## Implementation Steps

### 1. Add Link from Process Page

**File:** `analytics-web-app/src/app/process/page.tsx`

Add a "View Metrics" button in the header next to "View Log" and "Generate Trace":

```tsx
<Link
  href={`/process_metrics?process_id=${processId}&from=${encodeURIComponent(timeRange.from)}&to=${encodeURIComponent(timeRange.to)}`}
  className="flex items-center gap-2 px-4 py-2 bg-theme-border text-theme-text-primary rounded-md hover:bg-theme-border-hover transition-colors text-sm"
>
  <BarChart2 className="w-4 h-4" />
  View Metrics
</Link>
```

### 2. Create Metrics Page

**File:** `analytics-web-app/src/app/process_metrics/page.tsx`

#### Step 1: Discover Available Measures

On page load, query the distinct measure names for this process:

```sql
SELECT name, first_value(target) as target, first_value(unit) as unit
FROM view_instance('measures', '$process_id')
GROUP BY name
ORDER BY name
```

This populates a dropdown selector for the user to choose which measure to view.

#### Step 2: Query Selected Measure

**Default SQL:**
```sql
SELECT
  date_bin('$bin_interval', time) as time,
  max(value) as value
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
GROUP BY date_bin('$bin_interval', time)
ORDER BY time
```

Uses DataFusion's [`date_bin`](https://datafusion.apache.org/user-guide/sql/scalar_functions.html#date_bin) to downsample data to a reasonable number of points for display.

**Variables:**
- `process_id` - Current process ID
- `measure_name` - Selected measure name (from dropdown)
- `bin_interval` - Time bucket size (e.g., '1 second', '100 milliseconds')

**Bin Interval Calculation:**
```
num_bins = chart_width_pixels / pixels_per_bin
bin_interval_ms = time_span_ms / num_bins
```
- Use ~3 pixels per bin for good visual density
- Example: 800px chart width → ~267 bins
- Round to sensible intervals (1ms, 10ms, 100ms, 1s, 10s, etc.)

#### UI Components:

- **Measure selector dropdown** - populated from discovery query, shows `name (unit)` format
- **Data points count** indicator

#### Display:

- **Time-series line chart** (Grafana-style)
  - Line with filled area underneath
  - Y-axis with auto-scaled value labels
  - X-axis with time labels
  - Crosshair + tooltip on hover showing exact time and value
- **Chart header** with measure name and unit

#### Empty States:

Two empty state scenarios (see `tasks/process-metrics-empty-mockup.html`):

1. **No measures available** - Discovery query returns zero rows
   - Disable measure dropdown with "No measures available"
   - Show centered message: "No measures for the selected time range" with suggestion to expand time range

2. **No data in time range** - Measure selected but data query returns zero rows
   - Show measure dropdown normally
   - Display "0 data points" count
   - Show chart header with measure name
   - Centered message: "No data in time range" with suggestion to expand time range

### 3. Components and Utilities

Reuse existing patterns from the log screen:

- `PageLayout` with right panel for QueryEditor
- `QueryEditor` component with variables documentation
- `useTimeRange` hook for time range management
- `executeSqlQuery` API function

**New component needed:**
- `TimeSeriesChart` - uPlot-based line chart (see Charting Technology section below)

## Charting Technology

### Library: uPlot + uplot-react

**Dependencies to add:**
```bash
yarn add uplot uplot-react
```

**Why uPlot:**
- **Performance**: Canvas-based rendering handles 100K+ data points without slowdown
- **Bundle size**: ~35KB total (uPlot + wrapper)
- **Purpose-built**: Designed specifically for time-series visualization
- **Active maintenance**: Regular updates through 2025

**Why uplot-react wrapper:**
- Declarative React props instead of imperative API
- Smart instance management - updates in place rather than recreating on every prop change
- ~30K weekly downloads, no known vulnerabilities

### TimeSeriesChart Component

**File:** `analytics-web-app/src/components/TimeSeriesChart.tsx`

```tsx
import UplotReact from 'uplot-react';
import uPlot from 'uplot';
import 'uplot/dist/uPlot.min.css';

interface TimeSeriesChartProps {
  data: { time: number; value: number }[];
  title: string;
  unit: string;
}

export function TimeSeriesChart({ data, title, unit }: TimeSeriesChartProps) {
  // Transform data to uPlot format: [[timestamps], [values]]
  const uplotData: uPlot.AlignedData = [
    data.map(d => d.time / 1000), // uPlot uses seconds
    data.map(d => d.value),
  ];

  const options: uPlot.Options = {
    width: 800,  // Will be made responsive
    height: 300,
    title,
    series: [
      {},  // x-axis (time)
      {
        label: title,
        stroke: '#73bf69',
        fill: 'rgba(115, 191, 105, 0.1)',
        width: 2,
      },
    ],
    axes: [
      { /* x-axis time formatting */ },
      { label: unit },
    ],
    cursor: {
      show: true,
      x: true,
      y: true,
    },
    legend: { show: true },
  };

  return <UplotReact options={options} data={uplotData} />;
}
```

### Features provided by uPlot:
- Auto-scaling axes
- Hover crosshair and tooltip (built-in cursor plugin)
- Responsive sizing (via ResizeObserver or manual width updates)
- Zoom/pan support (optional plugins)
- Legend with series toggle

## Implementation Steps (continued)

### 4. URL Parameters

Route: `/process_metrics`

Query parameters:
- `process_id` (required) - The process UUID
- `from` / `to` - Time range (inherited from global time range)
- `measure` - Selected measure name (persisted in URL)

### 5. Workflow

1. User clicks "View Metrics" from process page
2. Page loads and runs discovery query to get available measure names
3. User selects a measure from dropdown (or first one selected by default)
4. Page queries `SELECT time, value ... WHERE name = '$measure_name'`
5. Results displayed as time-series chart
6. User can modify SQL in QueryEditor panel for custom queries

## File Changes Summary

| File | Change |
|------|--------|
| `analytics-web-app/src/app/process/page.tsx` | Add "View Metrics" link |
| `analytics-web-app/src/app/process_metrics/page.tsx` | New file - metrics viewer page |
| `analytics-web-app/src/components/TimeSeriesChart.tsx` | New file - chart component |

## Page Mockups

- `tasks/process-metrics-mockup.html` - Interactive mockup with chart and data
- `tasks/process-metrics-empty-mockup.html` - Empty state scenarios

**Key elements:**
- Measure dropdown with name + unit (e.g., "frame_time (ms)")
- Grafana-style time-series line chart with filled area
- Chart header with measure name and unit
- Crosshair + tooltip on hover
- SQL panel on right side (toggleable)

## Future Enhancements (Out of Scope)

- Aggregation queries (avg, min, max over time windows)
- Multi-measure overlay comparison
- Properties display/filtering
- Zoom/pan controls on chart
