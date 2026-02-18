# Multi-Query Chart Cell Plan

**GitHub Issue**: #749

## Overview

Add support for executing multiple SQL queries within a single notebook chart cell, allowing users to overlay data from different queries in a single visualization. Each query produces a named series. Different queries may return data with different units — the Y axis scales independently per unit, with separate axes on left and right.

## Current State

Chart cells execute a single SQL query and render one series:

- **Config**: `QueryCellConfig` has `sql: string` and `options?: Record<string, unknown>` where options stores `scale_mode`, `chart_type`, and `unit`
- **Execution**: `chartMetadata.execute()` calls `runQuery(sql)` once, returns `{ data: Table }` (`ChartCell.tsx:207-211`)
- **Data extraction**: `extractChartData()` in `arrow-utils.ts` validates exactly 2 columns (X, Y) and returns `{ x: number; y: number }[]`
- **Rendering**: `XYChart` component uses uPlot with a single Y-axis and single series, hardcoded to `#bf360c` (rust orange)
- **State**: `CellState.data` holds a single `Table | null`

Key files:
- `analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx` — cell metadata, renderer, editor
- `analytics-web-app/src/components/XYChart.tsx` — uPlot chart component (663 lines)
- `analytics-web-app/src/lib/arrow-utils.ts` — `extractChartData()`, `validateChartColumns()`
- `analytics-web-app/src/lib/screen-renderers/notebook-types.ts` — `QueryCellConfig`, `CellState`
- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts` — query execution

## Design

### Config Schema

Use a `version` field to distinguish between legacy single-query configs and the new multi-query format. All queries live at the same level in a `queries` array.

```typescript
// v1 (implicit — current format, no version field)
interface ChartCellConfigV1 extends CellConfigBase {
  type: 'chart'
  sql: string
  options?: {
    unit?: string
    scale_mode?: ScaleMode
    chart_type?: ChartType
  }
  dataSource?: string
}

// v2 — all queries are peers in an array, each with its own data source
interface ChartCellConfigV2 extends CellConfigBase {
  type: 'chart'
  version: 2
  queries: ChartQueryDef[]            // Each query has its own dataSource
  options?: {
    scale_mode?: ScaleMode
    chart_type?: ChartType
  }
}

interface ChartQueryDef {
  sql: string
  unit?: string                       // Y-axis unit (e.g., "bytes", "ms", "percent")
  label?: string                      // Legend label (defaults to Y column name)
  dataSource: string                  // Per-query data source (required, set from default data source on creation)
}
```

**Migration**: On load, if no `version` field exists, convert in-memory to v2 format by wrapping the single `sql` + `options.unit` into `queries[0]`. On save, always write v2 format. This is a one-way migration — once a chart is saved, it uses v2.

```typescript
function migrateChartConfig(config: CellConfig): ChartCellConfigV2 {
  if ('version' in config && config.version === 2) {
    return config as ChartCellConfigV2  // Already v2
  }
  const v1 = config as ChartCellConfigV1
  return {
    ...v1,
    version: 2,
    queries: [{
      sql: v1.sql,
      unit: v1.options?.unit as string | undefined,
      dataSource: v1.dataSource,
    }],
    options: {
      scale_mode: v1.options?.scale_mode,
      chart_type: v1.options?.chart_type,
    },
  }
}
```

Since `QueryCellConfig` has `options?: Record<string, unknown>`, the `version` and `queries` fields can coexist on the config object without changing the shared type. The chart cell's execute/render methods handle the v2 shape internally.

### Execution

The chart `execute()` method migrates to v2, then runs all queries. Since each query can target a different data source, the existing `CellExecutionContext.runQuery(sql)` isn't sufficient — it resolves data source once per cell. We extend the context:

```typescript
export interface CellExecutionContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>
  runQueryWithDataSource?: (sql: string, dataSource?: string) => Promise<Table>
  registerTable?: (ipcBytes: Uint8Array) => void
}
```

`runQueryWithDataSource` resolves data source variables and routes accordingly. The existing `runQuery` is unchanged (backward compat for all other cell types). `useCellExecution.ts` builds the new function using the same fetch/WASM logic but with a per-call data source override.

```typescript
execute: async (config, { variables, timeRange, runQuery, runQueryWithDataSource }) => {
  const v2 = migrateChartConfig(config)
  const run = runQueryWithDataSource ?? runQuery

  // First query — registered in WASM as cell name
  const firstSql = substituteMacros(v2.queries[0].sql, variables, timeRange)
  const data = await run(firstSql, v2.queries[0].dataSource)

  if (v2.queries.length === 1) {
    return { data }
  }

  // Additional queries — run sequentially
  const additionalData: Table[] = []
  for (let i = 1; i < v2.queries.length; i++) {
    const sql = substituteMacros(v2.queries[i].sql, variables, timeRange)
    const table = await run(sql, v2.queries[i].dataSource)
    additionalData.push(table)
  }

  return { data, additionalData }
}
```

**WASM registration**: Only the first query registers as the cell name (for cross-cell references). Additional queries exist only for visualization overlay.

### CellState Extension

```typescript
export interface CellState {
  // ... existing fields ...
  /** Additional query results for multi-query cells (e.g., chart overlays) */
  additionalData?: Table[]
}
```

### Data Extraction

New function in `arrow-utils.ts`:

```typescript
export interface ChartSeriesData {
  label: string
  unit: string
  data: { x: number; y: number }[]
}

export interface MultiSeriesChartData {
  xAxisMode: XAxisMode
  xLabels?: string[]
  xColumnName: string
  series: ChartSeriesData[]
}

export function extractMultiSeriesChartData(
  tables: { table: Table; unit?: string; label?: string }[]
): { ok: true } & MultiSeriesChartData | { ok: false; error: string }
```

Each table must have 2 columns (X, Y). All tables must agree on X-axis mode (all time, or all categorical with same labels, or all numeric). The function validates this and returns an error if modes conflict.

### Multi-Axis Rendering

uPlot natively supports multiple Y-axes. Each series references a `scale`, and each axis references a `scale`. Series with the same unit share a scale/axis.

```
                    unit A          unit B
                   (left)          (right)
                      │               │
  ┌───────────────────┼───────────────┼───┐
  │  100 ms ──────────┤               ├── 500 bytes
  │                   │    ~~~        │
  │   50 ms ──────────┤  ~~~ ---     ├── 250 bytes
  │                   │~~~   ---     │
  │    0 ms ──────────┤       ---    ├──   0 bytes
  └───────────────────┴───────────────┴───┘
                    time →
```

- **Left axis**: first unit encountered
- **Right axis**: second unit encountered
- **Additional axes**: alternate sides (3rd on left, 4th on right, etc.)
- **Same unit**: series share the axis (no duplicate axes)
- **No unit**: series grouped under a shared "default" axis

uPlot config structure:

```typescript
scales: {
  x: { time: true },
  'ms': { ... },      // scale per unit
  'bytes': { ... },
}
axes: [
  xAxisConfig,
  { scale: 'ms', side: 1 },      // side 1 = left
  { scale: 'bytes', side: 3 },   // side 3 = right
]
series: [
  {},                              // X
  { scale: 'ms', stroke: '#bf360c', label: 'latency' },
  { scale: 'bytes', stroke: '#1565c0', label: 'response_size' },
]
```

### Color Palette

Use the brand's official 12-color chart sequence from `branding/extended-palette.md`:

```typescript
const SERIES_COLORS = [
  '#bf360c',  // Rust Orange
  '#1565c0',  // Cobalt Blue
  '#ffb300',  // Wheat
  '#2e7d32',  // Field Green
  '#5e35b1',  // Violet Dusk
  '#ff8f00',  // Harvest Gold
  '#00897b',  // Teal
  '#c62828',  // Crimson
  '#7e57c2',  // Lavender Storm
  '#827717',  // Olive Path
  '#00acc1',  // Cyan
  '#ad1457',  // Pink Dusk
]
```

Series are assigned colors in order. Single-query charts keep the existing rust orange (index 0). This palette is designed for dark backgrounds and provides 12 distinguishable colors before wrapping.

### XYChart Component Changes

Extend `XYChart` to accept multi-series data as an alternative to the existing single-series `data` prop:

```typescript
interface XYChartProps {
  // Existing single-series props (unchanged, backward compat)
  data?: { x: number; y: number }[]
  yColumnName?: string
  unit?: string

  // New: multi-series
  series?: ChartSeriesData[]

  // Shared props (unchanged)
  xAxisMode: XAxisMode
  xLabels?: string[]
  xColumnName?: string
  title?: string
  scaleMode?: ScaleMode
  onScaleModeChange?: (mode: ScaleMode) => void
  chartType?: ChartType
  onChartTypeChange?: (type: ChartType) => void
  onTimeRangeSelect?: (from: Date, to: Date) => void
}
```

When `series` is provided, use multi-series rendering. When `data` is provided (legacy), wrap as single series internally. The two are mutually exclusive.

### Tooltip

The existing tooltip plugin shows one value. For multi-series, show all series values at the cursor's X position:

```
  14:30:22.100
  ──────────────
  ● latency     123.4 ms
  ● resp_size   4.2 KB
```

uPlot's cursor snaps to the nearest X point per series. The tooltip iterates `u.data` for each series at `u.cursor.idx`.

### Legend

Replace the current single-series indicator in the chart header with a multi-series legend:

```
  latency (ms)   vs Time          min: 12.3 ms  p99: 234 ms  max: 1.2 s
  ● latency  ● response_size     [Line|Bar] [P99|Max]
```

Each legend item shows: color swatch + label. Stats (min/p99/max/avg) apply to the focused/first series.

**Legend interactivity** (Grafana-style):
- **Click**: Isolate that series (hide all others). Click again to restore all.
- **Ctrl+Click** (or Cmd on Mac): Toggle that single series without affecting others.

uPlot supports this natively via `setSeries(idx, { show: bool })`.

### Editor UI

All queries are listed uniformly. The editor always works with v2 format (migrating on open):

```
┌─ Query 1 ────────────────────────────────────┐
│ Data Source: [production (default)       ▾]  │
│ SELECT time, latency FROM measures ...       │
├──────────────────────────────────────────────┤
│ Unit: [ms       ]  Label: [latency         ] │
└──────────────────────────────────────────────┘

┌─ Query 2 ────────────────────────────── [✕] ─┐
│ Data Source: [staging                   ▾]   │
│ SELECT time, resp_bytes FROM measures ...    │
├──────────────────────────────────────────────┤
│ Unit: [bytes    ]  Label: [response_size   ] │
└──────────────────────────────────────────────┘

[+ Add Query]
```

Each query has:
- Data source selector (dropdown, required — initialized to default data source on creation)
- SQL editor (SyntaxEditor)
- Unit field (text input, supports `$variable.unit` macros)
- Label field (optional, defaults to Y column name)
- Remove button (✕) — hidden when only one query remains

## Implementation Steps

### Step 1: Config Migration
- Add `ChartQueryDef`, `ChartCellConfigV2` types and `migrateChartConfig()` function in `ChartCell.tsx`
- Migration: wrap v1 `sql` + `options.unit` + `dataSource` into `queries[0]`, set `version: 2`

### Step 2: CellState + Execution Context Extension
- Add `additionalData?: Table[]` to `CellState` in `notebook-types.ts`
- Add `runQueryWithDataSource?: (sql: string, dataSource?: string) => Promise<Table>` to `CellExecutionContext` in `cell-registry.ts`
- Update `useCellExecution.ts` to build `runQueryWithDataSource` (same fetch/WASM logic with per-call data source override) and propagate `additionalData` from execute result to cell state
- Add `additionalData?: Table[]` to `CellRendererProps` in `cell-registry.ts`

### Step 3: Multi-Series Data Extraction
- Add `ChartSeriesData` and `MultiSeriesChartData` types to `arrow-utils.ts`
- Add `extractMultiSeriesChartData()` function
- Keep existing `extractChartData()` unchanged (used by MetricsRenderer)

### Step 4: Chart Execution — Multiple Queries
- Update `chartMetadata.execute()` in `ChartCell.tsx` to use `migrateChartConfig()` and run all queries
- First query registers in WASM as cell name, additional queries run sequentially
- Return `{ data, additionalData }` when multiple queries exist

### Step 5: XYChart Multi-Series Rendering
- Add `series?: ChartSeriesData[]` prop to `XYChart`
- Add `SERIES_COLORS` palette
- Build uPlot config with multiple scales, axes, and series when `series` prop is provided
- Left axis for first unit, right axis for second unit
- Multi-series tooltip plugin
- Multi-series legend in header with Grafana-style click/ctrl+click interactivity
- Keep existing single-series code path when `data` prop is used

### Step 6: ChartCell Renderer Update
- Update `ChartCell` renderer to migrate config to v2
- Build series array from primary + additional data with per-query units
- Pass `series` prop to `XYChart`
- Update `chartMetadata.getRendererProps()` to pass additional data through

### Step 7: ChartCell Editor — Query Management
- Editor always works with v2 format (migrate on open)
- Uniform list of queries, each with data source selector, SQL editor, unit field, label field
- Data source selector per query (reuse `DataSourceSelector`, falls back to default data source)
- Remove button hidden when only one query remains
- "Add Query" button appends to `queries` array
- On save, write v2 format (version: 2, queries array)

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/notebook-types.ts` — `CellState` extension
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` — `CellRendererProps` extension
- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts` — propagate `additionalData`
- `analytics-web-app/src/lib/arrow-utils.ts` — `extractMultiSeriesChartData()`, types
- `analytics-web-app/src/components/XYChart.tsx` — multi-series rendering, tooltip, legend, colors
- `analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx` — execute, renderer, editor

## Trade-offs

### Versioned config with queries array
**Chosen**: Use a `version` field to transition from v1 (single `sql` field) to v2 (uniform `queries` array). All queries are peers at the same level. Migration happens in-memory on load; saving always writes v2. The `QueryCellConfig` shared type isn't changed — `version` and `queries` are extra fields on the config object (compatible with `Record<string, unknown>` indexing on `options`). Chart-specific code handles the v2 shape internally.

**Alternative**: Store additional queries in `options.queries` while keeping the primary in `sql`. Simpler migration but creates a two-tier query structure where the first query is special. The uniform array is easier to reason about in the editor and renderer.

### Shared X-axis requirement
**Chosen**: All queries in a chart must share the same X-axis mode (all time-based, all categorical, or all numeric). This simplifies rendering and is the natural use case — comparing different metrics over the same time range.

**Alternative**: Allow mixed X-axis modes. This would be significantly more complex and hard to visualize meaningfully.

### Unlimited Y-axes
**Chosen**: No cap on distinct Y-axes. uPlot supports multiple axes on each side. First unit goes on the left, second on the right, additional axes alternate or stack. Users can limit themselves by choosing compatible units.

**Alternative**: Cap at 2 (left + right). Simpler but artificially limits power users.

### Sequential query execution
**Chosen**: Run queries sequentially. This respects WASM engine state ordering and keeps abort logic simple.

**Alternative**: Parallel execution. Faster but complicates abort handling and WASM registration.

## Testing Strategy

- Unit tests for `extractMultiSeriesChartData()` — validate X-axis mode agreement, multi-unit grouping, error cases
- Unit tests for chart `execute()` — verify additional queries are run and results stored
- Manual testing:
  - Single-query charts render exactly as before (no regression)
  - Two queries with same unit: both series on shared Y-axis
  - Two queries with different units: left + right Y-axes with independent scales
  - Tooltip shows all series values
  - Legend shows all series with correct colors
  - Editor: add/remove queries, edit SQL/unit/label/dataSource
  - Two queries targeting different data sources route correctly
  - Time range drag-to-zoom works with multi-series
  - Scale mode (P99/Max) applies to all series
  - Chart type (Line/Bar) applies to all series

## Open Questions

None — all resolved.
