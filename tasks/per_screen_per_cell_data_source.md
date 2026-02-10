# Per-Screen and Per-Cell Data Source Selection

## Context

Data sources were recently added to the web app, but the source selector lives at the page level (ScreenPage header) and isn't persisted. This means:
- Changing data source reverts on reload
- All notebook cells share one data source
- The source can't be configured per-screen

**Goal**: Custom screens save their data source in config (via the editor pane). Notebook screens drop the global source entirely - individual SQL cells get their own data source selector in the cell editor.

## Changes

### 1. Add `dataSource` to SQL-executing cell config types

**File**: `analytics-web-app/src/lib/screen-renderers/notebook-types.ts`

Add `dataSource?: string` to `QueryCellConfig`, `VariableCellConfig`, and `PerfettoExportCellConfig` - all types that query data. Only `MarkdownCellConfig` doesn't get the field.

### 2. Per-cell data source in notebook execution

**File**: `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`

In `executeCell` (around line 126), read the cell's own data source with fallback. Since `dataSource` is on `QueryCellConfig` and `VariableCellConfig` (not the base), access it with a type check or cast:
```ts
const cellDataSource = ('dataSource' in cell ? cell.dataSource : undefined) || dataSource
```
Pass `cellDataSource` to `executeSql` instead of the global `dataSource`.

No interface changes - the hook's `dataSource` param becomes the default fallback.

### 3. Data source selector in cell editor

**File**: `analytics-web-app/src/components/CellEditor.tsx`

- Add `defaultDataSource?: string` to the local `CellEditorProps` interface
- After the "Cell Name" section, render `DataSourceSelector` for cells that execute SQL:
  ```ts
  const shouldShowDataSource = cell.type !== 'markdown' &&
    (cell.type !== 'variable' || (cell as VariableCellConfig).variableType === 'combobox')
  ```
- Value: `cell.dataSource || defaultDataSource || ''`
- onChange: `onUpdate({ dataSource: ds })`

### 4. Wire NotebookRenderer to pass default data source

**File**: `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`

Pass `defaultDataSource={dataSource}` to `<CellEditor>` (line 564).

### 5. Add `topContent` prop to QueryEditor

**File**: `analytics-web-app/src/components/QueryEditor.tsx`

Add optional `topContent?: React.ReactNode` prop. Render it at the top of the scrollable content area (line 95, before the SyntaxEditor), only when the panel is expanded.

### 6. Custom screen renderers - config-based data source

For each renderer: add `dataSource?: string` to its config interface, compute `effectiveDataSource = config.dataSource || props.dataSource`, use it for queries, pass `DataSourceSelector` as `topContent` to `QueryEditor`, and add a re-execution effect for data source changes.

**File**: `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx`
- Add `dataSource?: string` to `TableConfig`
- `const effectiveDataSource = tableConfig.dataSource || dataSource`
- Use `effectiveDataSource` in `executeQuery` callback and initial guard
- Pass `DataSourceSelector` as `topContent` to... TableRenderer has its own inline panel, not QueryEditor. So add the selector directly inside the expanded panel, above the "Query" section (around line 245).
- Add re-execution effect for `effectiveDataSource` changes

**File**: `analytics-web-app/src/lib/screen-renderers/LogRenderer.tsx`
- Add `dataSource?: string` to `LogConfig`
- `const effectiveDataSource = logConfig.dataSource || dataSource`
- Use in `loadData` callback
- Pass as `topContent` to `<QueryEditor>`
- Add re-execution effect

**File**: `analytics-web-app/src/lib/screen-renderers/MetricsRenderer.tsx`
- Add `dataSource?: string` to `MetricsConfig`
- `const effectiveDataSource = metricsConfig.dataSource || dataSource`
- Pass to `useScreenQuery({ ..., dataSource: effectiveDataSource })`
- Pass as `topContent` to `<QueryEditor>`
- Add re-execution effect in `useScreenQuery` (see step 7)

**File**: `analytics-web-app/src/lib/screen-renderers/ProcessListRenderer.tsx`
- Add `dataSource?: string` to `ProcessListConfig`
- Same pattern as TableRenderer

### 7. Add data source change re-execution to `useScreenQuery`

**File**: `analytics-web-app/src/lib/screen-renderers/useScreenQuery.ts`

Add an effect (similar to the time range change effect) that re-executes when `dataSource` changes:
```ts
const prevDataSourceRef = useRef<string | null>(null)
useEffect(() => {
  if (prevDataSourceRef.current === null) {
    prevDataSourceRef.current = dataSource || ''
    return
  }
  if (prevDataSourceRef.current !== (dataSource || '')) {
    prevDataSourceRef.current = dataSource || ''
    executeQuery(currentSqlRef.current)
  }
}, [dataSource, executeQuery])
```

This covers MetricsRenderer.

### 8. Add data source change re-execution to Table/Log/ProcessList renderers

Table, Log, and ProcessList don't use `useScreenQuery` — they call `streamQuery` directly and guard initial execution with `hasExecutedRef`. That guard prevents re-execution when the data source changes after first load, so each renderer needs a separate effect:

```ts
const prevDataSourceRef = useRef<string | null>(null)
useEffect(() => {
  if (prevDataSourceRef.current === null) {
    prevDataSourceRef.current = effectiveDataSource || ''
    return
  }
  if (prevDataSourceRef.current !== (effectiveDataSource || '')) {
    prevDataSourceRef.current = effectiveDataSource || ''
    executeQuery(currentSql)  // or loadData(currentSql) for LogRenderer
  }
}, [effectiveDataSource, executeQuery])
```

Add this to `TableRenderer.tsx`, `LogRenderer.tsx`, and `ProcessListRenderer.tsx`. The callbacks (`executeQuery`/`loadData`) must close over `effectiveDataSource` (not the prop-level `dataSource`) so the re-executed query hits the right source.

### 9. Remove page-level DataSourceSelector

**File**: `analytics-web-app/src/routes/ScreenPage.tsx`

- Remove `<DataSourceSelector value={dataSource} onChange={setDataSource} />` from the header (line 380)
- Remove `DataSourceSelector` import
- Keep `useDefaultDataSource()`, `dataSource` state, and the effect that sets it - these provide the fallback value passed to renderers via `dataSource={dataSource}` prop

### 10. DataSourceSelector: always show in editor context

**File**: `analytics-web-app/src/components/DataSourceSelector.tsx`

Currently hides when there's only 1 data source. In editor panes, users should see which source is active even with a single source. Add `showWithSingleSource?: boolean` prop - when true, render even with 1 source. Cell editors and renderer panels pass `showWithSingleSource`.

## Files modified (summary)

| File | Change |
|------|--------|
| `notebook-types.ts` | Add `dataSource?` to `QueryCellConfig`, `VariableCellConfig`, and `PerfettoExportCellConfig` |
| `useCellExecution.ts` | Read per-cell `dataSource` with fallback |
| `CellEditor.tsx` | Add `defaultDataSource` prop, conditional selector |
| `NotebookRenderer.tsx` | Pass `defaultDataSource` to CellEditor |
| `QueryEditor.tsx` | Add `topContent` prop |
| `DataSourceSelector.tsx` | Add `alwaysShow` prop |
| `TableRenderer.tsx` | Config-based data source + selector in panel |
| `LogRenderer.tsx` | Config-based data source + selector via QueryEditor |
| `MetricsRenderer.tsx` | Config-based data source + selector via QueryEditor |
| `ProcessListRenderer.tsx` | Config-based data source + selector via QueryEditor |
| `useScreenQuery.ts` | Data source change re-execution effect |
| `ScreenPage.tsx` | Remove header-level DataSourceSelector |

## What stays the same

- `ScreenRendererProps.dataSource` prop name and type unchanged (becomes "default/fallback")
- Backend API unchanged - `data_source` in query stream body works as before
- `getDataSourceList()` caching unchanged
- Screen save/load unchanged - `ScreenConfig` already has `[key: string]: unknown`

## Verification

1. `yarn type-check` - verify no type errors
2. `yarn lint` - verify no lint issues
3. `yarn test` - run existing tests
4. Manual testing:
   - Open a custom screen (table/log/metrics/process-list) - data source selector should appear in the editor pane, not the header
   - Change data source in editor pane - query should re-execute with new source
   - Save the screen - data source should persist on reload
   - Open a notebook - no global data source selector in header
   - Click a SQL cell (table/chart/log) - data source selector in cell editor
   - Different cells can have different data sources
   - Variable (combobox) cell shows data source selector; text/markdown cells do not
   - With only 1 data source configured, selector still shows in editors (`alwaysShow`)
