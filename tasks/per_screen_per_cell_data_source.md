# Per-Screen and Per-Cell Data Source Selection

**Status**: Implemented on `source` branch.

## Context

Data sources were recently added to the web app, but the source selector lives at the page level (ScreenPage header) and isn't persisted. This means:
- Changing data source reverts on reload
- All notebook cells share one data source
- The source can't be configured per-screen

**Goal**: Custom screens save their data source in config (via the editor pane). Notebook screens drop the global source entirely - individual SQL cells get their own data source selector in the cell editor.

## Implementation

### 1. Add `dataSource` to SQL-executing cell config types

**File**: `analytics-web-app/src/lib/screen-renderers/notebook-types.ts`

Added `dataSource?: string` to `QueryCellConfig`, `VariableCellConfig`, and `PerfettoExportCellConfig` - all types that query data. Only `MarkdownCellConfig` doesn't get the field.

### 2. Per-cell data source in notebook execution

**File**: `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`

In `executeCell`, reads the cell's own data source with fallback:
```ts
const cellDataSource = ('dataSource' in cell ? cell.dataSource : undefined) || dataSource
```
Passes `cellDataSource` to `executeSql` instead of the global `dataSource`.

### 3. Data source selector in cell editor

**File**: `analytics-web-app/src/components/CellEditor.tsx`

- Added `defaultDataSource?: string` to `CellEditorProps`
- After the "Cell Name" section, renders `DataSourceSelector` for cells that execute SQL:
  ```ts
  const shouldShowDataSource = cell.type !== 'markdown' &&
    (cell.type !== 'variable' || (cell as VariableCellConfig).variableType === 'combobox')
  ```
- Value: `cell.dataSource || defaultDataSource || ''`
- onChange: `onUpdate({ dataSource: ds })`

### 4. Wire NotebookRenderer to pass default data source and per-cell effective data source

**File**: `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`

- Passes `defaultDataSource={dataSource}` to `<CellEditor>`
- Computes effective data source per cell (`cell.dataSource || dataSource`) and passes it to all cell renderers via the `dataSource` prop on `CellRendererProps`

### 5. `dataSource` on `CellRendererProps`

**File**: `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`

Added `dataSource?: string` to `CellRendererProps` so all cell renderers have access to their effective data source.

### 6. Add `topContent` prop to QueryEditor

**File**: `analytics-web-app/src/components/QueryEditor.tsx`

Added optional `topContent?: React.ReactNode` prop. Rendered at the top of the scrollable content area (before the SyntaxEditor), only when the panel is expanded.

### 7. Custom screen renderers - config-based data source

For each renderer: added `dataSource?: string` to its config interface, computed `effectiveDataSource = config.dataSource || props.dataSource`, used it for queries, added `DataSourceSelector` in the editor panel, and used `useChangeEffect` for re-execution on data source changes.

**File**: `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx`
- Added `dataSource?: string` to `TableConfig`
- `const effectiveDataSource = tableConfig.dataSource || dataSource`
- Used `effectiveDataSource` in `executeQuery` callback and initial guard
- Added `DataSourceSelector` directly inside the expanded panel, above the "Query" section (TableRenderer has its own inline panel, not QueryEditor)
- Re-execution via `useChangeEffect(effectiveDataSource, ...)`

**File**: `analytics-web-app/src/lib/screen-renderers/LogRenderer.tsx`
- Added `dataSource?: string` to `LogConfig`
- `const effectiveDataSource = logConfig.dataSource || dataSource`
- Used in `loadData` callback
- Passed as `topContent` to `<QueryEditor>`
- Re-execution via `useChangeEffect(effectiveDataSource, ...)`

**File**: `analytics-web-app/src/lib/screen-renderers/MetricsRenderer.tsx`
- Added `dataSource?: string` to `MetricsConfig`
- `const effectiveDataSource = metricsConfig.dataSource || dataSource`
- Passed to `useScreenQuery({ ..., dataSource: effectiveDataSource })`
- Passed as `topContent` to `<QueryEditor>`
- Re-execution handled by `useScreenQuery` (see step 8)

**File**: `analytics-web-app/src/lib/screen-renderers/ProcessListRenderer.tsx`
- Added `dataSource?: string` to `ProcessListConfig`
- Same pattern as TableRenderer (effectiveDataSource, selector via topContent, re-execution via `useChangeEffect`)

### 8. `useChangeEffect` hook — deduplicated re-execution pattern

**File**: `analytics-web-app/src/hooks/useChangeEffect.ts`

Extracted a reusable hook that runs a callback when a string value changes, skipping the initial render. Uses a ref for the callback so the effect only re-fires on value changes.

Used in `useScreenQuery`, `LogRenderer`, `ProcessListRenderer`, and `TableRenderer` to replace duplicated `prevDataSourceRef` + `useEffect` blocks.

### 9. `useDataSourceState` hook — deduplicated mutable data source pattern

**File**: `analytics-web-app/src/hooks/useDataSourceState.ts`

Extracted a reusable hook that wraps `useDefaultDataSource` with mutable local state. Initializes from the default data source, then allows the user to override via `setDataSource`.

Used in `ProcessesPage`, `ProcessLogPage`, and `ProcessMetricsPage` to replace duplicated `useState('')` + `useEffect` sync blocks.

### 10. Process pages — data source selector

**Files**: `ProcessesPage.tsx`, `ProcessLogPage.tsx`, `ProcessMetricsPage.tsx`

Each page uses `useDataSourceState()` and renders a `DataSourceSelector` as `topContent` in their `QueryEditor`. The mutable state allows users to switch data sources on these non-configurable pages.

### 11. Perfetto export — data source threading

**Files**: `PerfettoExportCell.tsx`, `perfetto-trace.ts`

- `PerfettoExportCell` reads `dataSource` from `CellRendererProps` and passes it to `fetchPerfettoTrace`
- `fetchPerfettoTrace` accepts `dataSource` in its options and forwards it to `streamQuery`
- Cache is cleared when `dataSource` changes

### 12. Remove page-level DataSourceSelector

**File**: `analytics-web-app/src/routes/ScreenPage.tsx`

- Removed `<DataSourceSelector>` from the header
- Uses `useDefaultDataSource()` directly (read-only) to provide the fallback value to renderers

### 13. DataSourceSelector auto-hides with single source

**File**: `analytics-web-app/src/components/DataSourceSelector.tsx`

The selector auto-hides when there is only one data source configured. This avoids showing a useless dropdown in single-source deployments.

### 14. Test updates

**File**: `analytics-web-app/src/lib/screen-renderers/__tests__/NotebookRenderer.test.tsx`

- Added `Database` and `AlertCircle` to the `lucide-react` icon mock (used by DataSourceSelector)
- Added mock for `@/lib/data-sources-api` with a never-settling promise to avoid act() warnings (these tests don't exercise data source selection)

## Files modified (summary)

| File | Change |
|------|--------|
| `notebook-types.ts` | Added `dataSource?` to `QueryCellConfig`, `VariableCellConfig`, `PerfettoExportCellConfig` |
| `cell-registry.ts` | Added `dataSource?` to `CellRendererProps` |
| `useCellExecution.ts` | Read per-cell `dataSource` with fallback |
| `CellEditor.tsx` | Added `defaultDataSource` prop, conditional selector |
| `NotebookRenderer.tsx` | Pass `defaultDataSource` to CellEditor, per-cell `dataSource` to renderers |
| `QueryEditor.tsx` | Added `topContent` prop |
| `DataSourceSelector.tsx` | Auto-hides with ≤1 source (removed `showWithSingleSource`) |
| `TableRenderer.tsx` | Config-based data source + selector in panel + `useChangeEffect` |
| `LogRenderer.tsx` | Config-based data source + selector via QueryEditor + `useChangeEffect` |
| `MetricsRenderer.tsx` | Config-based data source + selector via QueryEditor |
| `ProcessListRenderer.tsx` | Config-based data source + selector via QueryEditor + `useChangeEffect` |
| `useScreenQuery.ts` | Data source change re-execution via `useChangeEffect` |
| `useChangeEffect.ts` | New hook: run callback on string value change, skip initial render |
| `useDataSourceState.ts` | New hook: mutable data source state initialized from default |
| `perfetto-trace.ts` | Added `dataSource` to `FetchPerfettoTraceOptions`, forwarded to `streamQuery` |
| `PerfettoExportCell.tsx` | Reads `dataSource` from props, passes to `fetchPerfettoTrace`, clears cache on change |
| `ScreenPage.tsx` | Removed header-level DataSourceSelector, uses `useDefaultDataSource` directly |
| `ProcessesPage.tsx` | Uses `useDataSourceState`, added selector in QueryEditor |
| `ProcessLogPage.tsx` | Uses `useDataSourceState`, added selector in QueryEditor |
| `ProcessMetricsPage.tsx` | Uses `useDataSourceState`, added selector in QueryEditor |
| `NotebookRenderer.test.tsx` | Added icon mocks and data-sources-api mock (never-settling) |

## What stays the same

- `ScreenRendererProps.dataSource` prop name and type unchanged (becomes "default/fallback")
- Backend API unchanged - `data_source` in query stream body works as before
- `getDataSourceList()` caching unchanged
- Screen save/load unchanged - `ScreenConfig` already has `[key: string]: unknown`

## Verification

Automated (all passing):
1. `yarn type-check` - no type errors
2. `yarn lint` - no lint issues
3. `yarn test` - 644/644 tests passing

Manual testing checklist:
- [ ] Open a custom screen (table/log/metrics/process-list) - data source selector should appear in the editor pane, not the header
- [ ] Change data source in editor pane - query should re-execute with new source
- [ ] Save the screen - data source should persist on reload
- [ ] Open a notebook - no global data source selector in header
- [ ] Click a SQL cell (table/chart/log) - data source selector in cell editor
- [ ] Different cells can have different data sources
- [ ] Variable (combobox) cell shows data source selector; text/markdown cells do not
- [ ] With only 1 data source configured, selector auto-hides
- [ ] Perfetto export cell uses cell-level data source
