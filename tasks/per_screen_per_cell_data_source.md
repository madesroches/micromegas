# Per-Screen and Per-Cell Data Source Selection

**Status**: Implemented in `2ac54bd54` on `source` branch.

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

### 4. Wire NotebookRenderer to pass default data source

**File**: `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`

Passes `defaultDataSource={dataSource}` to `<CellEditor>`.

### 5. Add `topContent` prop to QueryEditor

**File**: `analytics-web-app/src/components/QueryEditor.tsx`

Added optional `topContent?: React.ReactNode` prop. Rendered at the top of the scrollable content area (before the SyntaxEditor), only when the panel is expanded.

### 6. Custom screen renderers - config-based data source

For each renderer: added `dataSource?: string` to its config interface, computed `effectiveDataSource = config.dataSource || props.dataSource`, used it for queries, added `DataSourceSelector` in the editor panel, and added a re-execution effect for data source changes.

**File**: `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx`
- Added `dataSource?: string` to `TableConfig`
- `const effectiveDataSource = tableConfig.dataSource || dataSource`
- Used `effectiveDataSource` in `executeQuery` callback and initial guard
- Added `DataSourceSelector` directly inside the expanded panel, above the "Query" section (TableRenderer has its own inline panel, not QueryEditor)
- Added re-execution effect for `effectiveDataSource` changes

**File**: `analytics-web-app/src/lib/screen-renderers/LogRenderer.tsx`
- Added `dataSource?: string` to `LogConfig`
- `const effectiveDataSource = logConfig.dataSource || dataSource`
- Used in `loadData` callback
- Passed as `topContent` to `<QueryEditor>`
- Added re-execution effect

**File**: `analytics-web-app/src/lib/screen-renderers/MetricsRenderer.tsx`
- Added `dataSource?: string` to `MetricsConfig`
- `const effectiveDataSource = metricsConfig.dataSource || dataSource`
- Passed to `useScreenQuery({ ..., dataSource: effectiveDataSource })`
- Passed as `topContent` to `<QueryEditor>`
- Re-execution handled by `useScreenQuery` (see step 7)

**File**: `analytics-web-app/src/lib/screen-renderers/ProcessListRenderer.tsx`
- Added `dataSource?: string` to `ProcessListConfig`
- Same pattern as TableRenderer (effectiveDataSource, selector via topContent, re-execution effect)

### 7. Add data source change re-execution to `useScreenQuery`

**File**: `analytics-web-app/src/lib/screen-renderers/useScreenQuery.ts`

Added an effect (similar to the time range change effect) that re-executes when `dataSource` changes. This covers MetricsRenderer.

### 8. Add data source change re-execution to Table/Log/ProcessList renderers

Table, Log, and ProcessList don't use `useScreenQuery` — they call `streamQuery` directly and guard initial execution with `hasExecutedRef`. Each renderer got a separate `prevDataSourceRef` + `useEffect` that re-executes when `effectiveDataSource` changes.

### 9. Remove page-level DataSourceSelector

**File**: `analytics-web-app/src/routes/ScreenPage.tsx`

- Removed `<DataSourceSelector value={dataSource} onChange={setDataSource} />` from the header
- Removed `DataSourceSelector` import
- Kept `useDefaultDataSource()`, `dataSource` state, and the effect that sets it - these provide the fallback value passed to renderers via `dataSource={dataSource}` prop

### 10. DataSourceSelector: always show in editor context

**File**: `analytics-web-app/src/components/DataSourceSelector.tsx`

Added `showWithSingleSource?: boolean` prop - when true, renders even with only 1 data source. Cell editors and renderer panels pass `showWithSingleSource`.

### 11. Test updates

**File**: `analytics-web-app/src/lib/screen-renderers/__tests__/NotebookRenderer.test.tsx`

- Added `Database` and `AlertCircle` to the `lucide-react` icon mock (used by DataSourceSelector)
- Added mock for `@/lib/data-sources-api` (`getDataSourceList` returns a single default source)

## Files modified (summary)

| File | Change |
|------|--------|
| `notebook-types.ts` | Added `dataSource?` to `QueryCellConfig`, `VariableCellConfig`, `PerfettoExportCellConfig` |
| `useCellExecution.ts` | Read per-cell `dataSource` with fallback |
| `CellEditor.tsx` | Added `defaultDataSource` prop, conditional selector |
| `NotebookRenderer.tsx` | Pass `defaultDataSource` to CellEditor |
| `QueryEditor.tsx` | Added `topContent` prop |
| `DataSourceSelector.tsx` | Added `showWithSingleSource` prop |
| `TableRenderer.tsx` | Config-based data source + selector in panel + re-execution |
| `LogRenderer.tsx` | Config-based data source + selector via QueryEditor + re-execution |
| `MetricsRenderer.tsx` | Config-based data source + selector via QueryEditor |
| `ProcessListRenderer.tsx` | Config-based data source + selector via QueryEditor + re-execution |
| `useScreenQuery.ts` | Data source change re-execution effect |
| `ScreenPage.tsx` | Removed header-level DataSourceSelector |
| `NotebookRenderer.test.tsx` | Added icon mocks and data-sources-api mock |

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
- [ ] With only 1 data source configured, selector still shows in editors (`showWithSingleSource`)
