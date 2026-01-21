# Plan: Generalize Metrics Screen for 2-Column Charts

**Status: ✅ Completed** (January 2025)

## Goal
Extend the metrics screen to chart any SQL query with 2 columns. First column is X-axis, second column is Y-axis. No column selection UI needed - column order determines axis mapping.

## Current State
- **MetricsRenderer.tsx** (lines 111-129): Hardcodes `row.time` and `row.value` column access
- **TimeSeriesChart.tsx** (line 247): Hardcodes `scales.x.time: true` for time-based X-axis
- uPlot supports non-time X-axis via `scales.x.time: false`

## Design Decisions
- **Column mapping**: First column = X, Second column = Y (no UI needed)
- **X-axis types**: Time, Numeric, and Categorical (strings)
- **Type detection**: Auto-detect from Arrow schema via `table.schema.fields[n].type`
- **Sorting**: Time/Numeric → sorted by X ascending (uPlot requirement). Categorical → preserve SQL order

## Implementation

### 1. Rename and Generalize TimeSeriesChart
**Rename:** `src/components/TimeSeriesChart.tsx` → `src/components/XYChart.tsx`

Generalize the existing component in place to avoid code duplication:

```typescript
type XAxisMode = 'time' | 'numeric' | 'categorical'

interface XYChartProps {
  data: { x: number; y: number }[]  // categorical: x is index into xLabels
  xAxisMode: XAxisMode  // required, determined by extractChartData
  xLabels?: string[]  // for categorical mode - the actual string labels
  xColumnName?: string
  yColumnName?: string
  // ... existing props: scaleMode, onScaleModeChange, onTimeRangeSelect
}
```

Key changes:
- Line 247: `scales.x.time: xAxisMode === 'time'`
- Tooltip: Format X as timestamp (time) / plain number (numeric) / label lookup (categorical)
- X-axis ticks: Date formatting (time) / number formatting (numeric) / `xLabels[Math.round(val)]` (categorical)
- Drag-to-select: Only enabled for time mode (drills down on time range)

### 2. Add Chart Data Utilities to arrow-utils.ts
**Modify:** `src/lib/arrow-utils.ts`

```typescript
type XAxisMode = 'time' | 'numeric' | 'categorical'

// Validate table has exactly 2 columns with correct types
function validateChartColumns(table: Table):
  | { valid: true; xType: DataType; yType: DataType }
  | { valid: false; error: string }

// Detect X-axis mode from Arrow column type
function detectXAxisMode(dataType: DataType): XAxisMode
// - Timestamp*, Date*, Date32, Date64 → 'time'
// - Int*, UInt*, Float* → 'numeric'
// - Utf8, LargeUtf8 → 'categorical'

// Type classification helpers
function isTimeType(dataType: DataType): boolean  // Timestamp*, Date*, Date32, Date64
function isNumericType(dataType: DataType): boolean
function isStringType(dataType: DataType): boolean

// Extract chart data from Arrow table (first 2 columns)
function extractChartData(table: Table):
  | {
      ok: true
      data: { x: number; y: number }[]
      xAxisMode: XAxisMode
      xLabels?: string[]  // for categorical - unique labels in SQL order
      xColumnName: string
      yColumnName: string
    }
  | { ok: false; error: string }
```

Validation rules:
- Exactly 2 columns required
- Column 1 (X): Must be timestamp, numeric, or string type
- Column 2 (Y): Must be numeric type
- Rows with null X or Y values are skipped
- Sorting: time/numeric → sorted by X ascending. Categorical → preserve SQL order

Categorical extraction:
```typescript
// Build label array, map strings to indices (preserves SQL order)
const labelMap = new Map<string, number>()
const xLabels: string[] = []
for (const row of rows) {
  const str = String(row[xCol])
  if (!labelMap.has(str)) {
    labelMap.set(str, xLabels.length)
    xLabels.push(str)
  }
  data.push({ x: labelMap.get(str)!, y: Number(row[yCol]) })
}
```

### 3. Update MetricsRenderer
**Modify:** `src/lib/screen-renderers/MetricsRenderer.tsx`

Replace hardcoded column access with utility function:

```typescript
// Before (lines 111-129)
const time = timestampToMs(row.time)
const value = Number(row.value)

// After
const chartResult = useMemo(() => extractChartData(query.table), [query.table])
```

Handle extraction result:
```tsx
if (!chartResult.ok) {
  return <EmptyState message={chartResult.error} />
}

const { data, xAxisMode, xLabels, xColumnName, yColumnName } = chartResult
```

Update imports and component usage:
```tsx
import { XYChart } from '@/components/XYChart'

<XYChart
  data={data}
  xAxisMode={xAxisMode}
  xLabels={xLabels}
  xColumnName={xColumnName}
  yColumnName={yColumnName}
  scaleMode={scaleMode}
  onScaleModeChange={handleScaleModeChange}
  onTimeRangeSelect={xAxisMode === 'time' ? handleTimeRangeSelect : undefined}
/>
```

### 4. Update Imports Across Codebase
Find and update all imports of `TimeSeriesChart` to use `XYChart`.

## Files to Modify

| File | Action |
|------|--------|
| `src/components/TimeSeriesChart.tsx` | Rename to `XYChart.tsx`, generalize |
| `src/lib/arrow-utils.ts` | Add validation + extraction utilities |
| `src/lib/screen-renderers/MetricsRenderer.tsx` | Use new utilities, update import |
| `src/components/MetricsChart.tsx` | Update imports: `TimeSeriesChart`, `ChartAxisBounds`, `ScaleMode` → from `XYChart` |
| `src/components/ThreadCoverageTimeline.tsx` | Update import: `ChartAxisBounds` → from `XYChart` |
| `src/components/PropertyTimeline.tsx` | Update import: `ChartAxisBounds` → from `XYChart` |
| `src/routes/PerformanceAnalysisPage.tsx` | Update import: `ChartAxisBounds` → from `XYChart` |

## Error Messages

| Condition | Message |
|-----------|---------|
| Column count ≠ 2 | "Query must return exactly 2 columns (X and Y axis)" |
| X column unsupported type | "First column must be timestamp, numeric, or string type for X-axis" |
| Y column not numeric | "Second column must be numeric type for Y-axis" |
| All rows filtered (nulls) | "No valid data points (all values are null)" |

## Test Cases

### Manual Verification
```sql
-- 1. Time series
SELECT time, value FROM metrics WHERE $__timeFilter(time)

-- 2. Numeric X-axis
SELECT cpu_percent, memory_mb FROM system_stats LIMIT 100

-- 3. Categorical X-axis
SELECT status, count(*) as cnt FROM requests GROUP BY status

-- 4. Categorical with ORDER BY (verify order preserved)
SELECT level, count(*) as cnt FROM logs GROUP BY level ORDER BY cnt DESC

-- 5. Error: wrong column count
SELECT time, value, extra FROM metrics  -- should error

-- 6. Error: non-numeric Y
SELECT time, status_text FROM logs  -- should error

-- 7. Nulls handled
SELECT time, value FROM metrics WHERE value IS NOT NULL OR value IS NULL
```

## Future Work (Deferred)
- **Multiple Y series**: Support 3+ columns for multi-line charts
- **Axis labels**: Display column names on axes
