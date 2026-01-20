# Histogram Visualization Implementation Plan

**Goal**: Show distribution of CPU usage in 100 buckets between 0 and 100, visualized as a bar chart in the analytics web app.

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/715

**Status**: IMPLEMENTED (branch: `histogram-visualization`)

## Implementation Summary

All planned steps have been completed:

| Step | Status | Notes |
|------|--------|-------|
| Create `expand_histogram` UDTF | Done | Used `TableFunctionImpl` pattern (matches codebase) |
| Export in mod.rs | Done | |
| Register in query.rs | Done | Line ~183 |
| Add bar chart to XYChart | Done | Using `uPlot.paths.bars()` |
| Add Line/Bar toggle UI | Done | Next to P99/Max toggle |
| Wire up MetricsRenderer | Done | Persists `chart_type` to config |
| Code review | Done | Fixed edge case: division by zero when `start == end` |

**Additional changes made during implementation:**
- Added edge case handling in `expand_histogram_to_batch()` for histograms where `start == end` (uses unit bin width)

**Not implemented:**
- Backend unit test for `expand_histogram` (existing histogram tests provide coverage for the pattern)

## Overview

The `make_histogram(start, end, bins, value)` UDAF already exists but returns a struct with a bins array. To display this as a bar chart, we need:
1. A way to expand histogram bins to chartable rows
2. Bar chart rendering support in the frontend

## Implementation Steps

### Step 1: Create `expand_histogram` UDTF (Backend)

Create a table-valued function that converts a histogram struct to rows of (bin_center, count).

**Files to create/modify:**
- Create `rust/analytics/src/dfext/histogram/expand.rs`
- Update `rust/analytics/src/dfext/histogram/mod.rs` to export it
- Register in `rust/analytics/src/lakehouse/query.rs`

**Function signature:**
```sql
expand_histogram(histogram) -> TABLE(bin_center FLOAT64, count UINT64)
```

**Implementation approach:**
- Use DataFusion's `TableFunctionImpl` trait (matches existing UDTF patterns in codebase)
- Input: histogram struct (same type returned by `make_histogram`)
- Output: rows with columns:
  - `bin_center`: f64 = start + (bin_index + 0.5) * bin_width
  - `count`: u64 = bins[bin_index]
- Edge case: when `start == end`, uses unit bin width to avoid division by zero

**Reference patterns:**
- Accessor UDFs in `rust/analytics/src/dfext/histogram/accessors.rs` show how to extract from HistogramArray
- `HistogramArray` methods: `get_start()`, `get_end()`, `get_bins()`

### Step 2: Add Bar Chart Support to XYChart (Frontend)

Extend XYChart to render bars instead of lines when configured.

**File to modify:** `analytics-web-app/src/components/XYChart.tsx`

**Changes:**
1. Add `chartType` prop:
```typescript
export type ChartType = 'line' | 'bar'

interface XYChartProps {
  // ... existing props
  chartType?: ChartType  // default: 'line'
}
```

2. Update series configuration to use `uPlot.paths.bars()` for bar type:
```typescript
series: [
  {},
  {
    label: title || yColumnName || 'Value',
    stroke: '#bf360c',
    fill: chartType === 'bar' ? 'rgba(191, 54, 12, 0.6)' : 'rgba(191, 54, 12, 0.1)',
    paths: chartType === 'bar'
      ? uPlot.paths.bars!({ size: [0.8], gap: 1 })
      : undefined,
    points: { show: chartType !== 'bar' },
  },
],
```

### Step 3: Add chart_type Option with UI Toggle (Frontend)

**Files to modify:**
- `analytics-web-app/src/components/XYChart.tsx` (add toggle UI)
- `analytics-web-app/src/lib/screen-renderers/MetricsRenderer.tsx` (pass through props)

**Changes:**

1. Add `chartType` and `onChartTypeChange` props to XYChart:
```typescript
interface XYChartProps {
  // ... existing props
  chartType?: ChartType
  onChartTypeChange?: (type: ChartType) => void
}
```

2. Add Line/Bar toggle button in chart header (next to P99/Max toggle):
```tsx
<div className="flex border border-theme-border rounded overflow-hidden">
  <button
    onClick={() => onChartTypeChange?.('line')}
    className={/* active/inactive styles */}
  >
    Line
  </button>
  <button
    onClick={() => onChartTypeChange?.('bar')}
    className={/* active/inactive styles */}
  >
    Bar
  </button>
</div>
```

3. Update MetricsRenderer to:
   - Read `chart_type` from config
   - Pass `chartType` and `onChartTypeChange` to XYChart
   - Persist chart type changes to config (like scale mode)

## SQL Query Patterns

### Basic Histogram Query

```sql
-- Create histogram of CPU usage (0-100%) with 100 buckets
SELECT bin_center, count
FROM expand_histogram(
  (SELECT make_histogram(0.0, 100.0, 100, value)
   FROM measures
   WHERE name = 'cpu_usage')
)
```

### Histogram for a Specific Process

```sql
SELECT bin_center, count
FROM expand_histogram(
  (SELECT make_histogram(0.0, 100.0, 100, value)
   FROM view_instance('measures', '{process_id}')
   WHERE name = 'cpu_usage')
)
```

### Histogram with Custom Range (e.g., frame time in ms)

```sql
-- Frame time distribution: 0-50ms in 50 buckets (1ms each)
SELECT bin_center, count
FROM expand_histogram(
  (SELECT make_histogram(0.0, 50.0, 50, value)
   FROM measures
   WHERE name = 'frame_time'
   AND unit = 'ms')
)
```

### How it Works

1. **Inner query**: `make_histogram(start, end, num_bins, value)` aggregates all `value` entries into a histogram struct
   - `start`: minimum value for bucketing (values below are clamped to first bucket)
   - `end`: maximum value for bucketing (values above are clamped to last bucket)
   - `num_bins`: number of buckets
   - `value`: the column to aggregate

2. **Outer query**: `expand_histogram(histogram)` converts the histogram struct to rows:
   - `bin_center`: the center value of each bucket (e.g., for bucket 0 in range 0-100 with 100 bins: center = 0.5)
   - `count`: number of values that fell into this bucket

The chart type can be toggled via the Line/Bar button in the chart header.

## Critical Files

| File | Action |
|------|--------|
| `rust/analytics/src/dfext/histogram/expand.rs` | Create (UDTF implementation) |
| `rust/analytics/src/dfext/histogram/mod.rs` | Modify (export expand module) |
| `rust/analytics/src/lakehouse/query.rs` | Modify (register UDTF at line ~181) |
| `analytics-web-app/src/components/XYChart.tsx` | Modify (add bar chart support + toggle UI) |
| `analytics-web-app/src/lib/screen-renderers/MetricsRenderer.tsx` | Modify (pass chart_type) |

## Verification

1. **Backend test**: Add test in `rust/analytics/tests/` that:
   - Creates a histogram with known values
   - Expands it and verifies bin_center and count values

2. **Frontend test**: Verify XYChart renders bars correctly

3. **End-to-end**:
   - Start services with `python3 local_test_env/ai_scripts/start_services.py`
   - Start web app with `cd analytics-web-app && yarn dev`
   - Create a metrics screen with the histogram SQL
   - Toggle to Bar chart type and verify visualization

## Code Review Process

After implementation, spawn an Explore agent to perform code review on the changed files:

**Review scope:**
- `rust/analytics/src/dfext/histogram/expand.rs`
- `rust/analytics/src/dfext/histogram/mod.rs`
- `rust/analytics/src/lakehouse/query.rs`
- `analytics-web-app/src/components/XYChart.tsx`
- `analytics-web-app/src/lib/screen-renderers/MetricsRenderer.tsx`

**Review criteria:**
- Code follows existing patterns in the codebase
- No security issues or obvious bugs
- Error handling is appropriate
- Code is readable and maintainable

**Iteration process:**
1. Apply review comments that fix actual issues
2. Ignore suggestions that diverge from the original goal (histogram visualization)
3. Keep refactorings reasonable - no scope creep
4. Re-run review until no blocking issues remain

**Out of scope for review fixes:**
- Unrelated code improvements
- Large refactorings of existing code
- Feature additions beyond histogram support
