# Notebook Variables as User-Selectable Data Sources

Issue: https://github.com/madesroches/micromegas/issues/792

## Context

Notebooks currently support `combobox`, `text`, and `expression` variable types. Query cells have a per-cell `dataSource` config field that's set to a hardcoded data source name. This feature adds a `datasource` variable type whose value is a data source name, and allows query cells to reference it via `$varname` syntax, enabling reusable notebooks that can be pointed at different environments without editing queries.

## Status: Complete

All implementation and tests are committed. 4 commits on `source` branch since `main`:

1. `0e710ce` Add datasource variable type for notebook data source selection
2. `0121fa6` Add data source dropdown for datasource variable default value
3. `24933e3` Pass datasourceVariables through CellEditorProps to type-specific editors
4. `98a90ff` Add tests for datasource variable type and $varname resolution

### What was done

**Step 1 - Type unions** (commit 1)
- `notebook-types.ts`: `'datasource'` added to `VariableCellConfig.variableType`
- `cell-registry.ts`: `'datasource'` added to `CellRendererProps.variableType`

**Step 2 - VariableCell.tsx implementation** (commits 1, 2, 3)
- Title bar: dropdown renders for datasource type (same as combobox)
- Execute: fetches from `getDataSourceList()`, returns `variableOptions`; catches API errors
- onExecutionComplete: reuses combobox validation (auto-select default/first if invalid)
- Editor: "Data Source" option in type selector, help text explaining `$varname` usage
- Editor: `DatasourceDefaultValue` component fetches sources for default value dropdown
- Editor: receives `datasourceVariables` and passes to its `DataSourceField`

**Step 3 - $varname resolution** (commit 1)
- `useCellExecution.ts`: resolves `$varname` in cell data source to variable value, falls back to notebook-level `dataSource`
- `NotebookRenderer.tsx`: same resolution in the render path

**Step 4 - DataSourceSelector.tsx** (commit 1)
- `datasourceVariables` prop on `DataSourceField` and `DataSourceSelector`
- Variable options in `<optgroup label="Variables">` with `$` prefix
- Selector shown when datasource variables exist (even with single data source)

**Step 5 - Wiring** (commits 1, 3)
- `NotebookRenderer.tsx`: computes `datasourceVarNames` from cells above, passes to `CellEditor`
- `CellEditor.tsx`: receives `datasourceVariables`, passes to `DataSourceField` and `meta.EditorComponent`
- `cell-registry.ts`: `datasourceVariables?: string[]` added to `CellEditorProps`

**Step 6 - Tests** (commit 4)
- `VariableCell.test.tsx` (14 new tests): datasource title bar (select dropdown, options, empty state, selection change); execute (API fetch, error handling, default label); onExecutionComplete (auto-select default, keep valid, replace invalid)
- `useCellExecution.test.ts` (3 new tests): `$varname` resolves to variable value, falls back on missing variable, literal dataSource unchanged

### Remaining

Manual verification only: Create a datasource variable, select a data source, reference `$varname` in a query cell's data source field, verify query executes against the selected source

## Edge Cases

- **Data source list empty/API error**: Execute catches errors, returns empty options. Dropdown shows "No options available".
- **Referenced variable not found**: Falls back to notebook-level `dataSource`.
- **Data source deleted**: `onExecutionComplete` detects value not in options, auto-selects default or first available.
- **Variable below referencing cell**: Only variables from cells above are available. Falls back to notebook-level data source.
- **No circular deps**: Datasource variables call the REST API, not `runQuery`, so no self-reference risk.

## Files Modified

1. `analytics-web-app/src/lib/screen-renderers/notebook-types.ts` - type union
2. `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` - type union + CellEditorProps
3. `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx` - core implementation
4. `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts` - $varname resolution
5. `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` - compute var names, resolve, pass to editor
6. `analytics-web-app/src/components/DataSourceSelector.tsx` - variable options in selector
7. `analytics-web-app/src/components/CellEditor.tsx` - pass datasourceVariables prop
