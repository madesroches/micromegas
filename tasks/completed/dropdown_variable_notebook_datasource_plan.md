# Dropdown Variable: Add Notebook Datasource Option

**Issue:** [#861](https://github.com/madesroches/micromegas/issues/861)

## Overview

When configuring a dropdown (combobox) variable cell in a notebook, the datasource selector does not include "notebook" as an option. This prevents dropdown variables from querying data produced by upstream cells. The fix is a one-line addition: pass `showNotebookOption={true}` to the `DataSourceField` component in the combobox editor.

## Current State

The `DataSourceSelector` component (`analytics-web-app/src/components/DataSourceSelector.tsx`) already supports an optional `showNotebookOption` prop. When `true`, it adds a "Notebook (local)" option at the top of the dropdown.

**Chart cells pass it** (`analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx:336`):
```tsx
<DataSourceSelector
  value={query.dataSource || defaultDataSource || ''}
  onChange={(ds) => updateQuery(i, { dataSource: ds })}
  datasourceVariables={datasourceVariables}
  showNotebookOption={true}
/>
```

**Variable cells do not** (`analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx:278-283`):
```tsx
<DataSourceField
  value={varConfig.dataSource || ''}
  onChange={(ds) => onChange({ ...varConfig, dataSource: ds })}
  datasourceVariables={datasourceVariables}
  className=""
/>
```

The downstream execution path already handles `"notebook"` datasources — `resolveCellDataSource` in `notebook-utils.ts:372-383` resolves it, and `useCellExecution.ts:145-157` routes notebook-sourced queries to the local WASM engine.

## Implementation Steps

1. **Add `showNotebookOption={true}` to the combobox datasource selector**

   **File:** `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx`, line 278

   Change:
   ```tsx
   <DataSourceField
     value={varConfig.dataSource || ''}
     onChange={(ds) => onChange({ ...varConfig, dataSource: ds })}
     datasourceVariables={datasourceVariables}
     className=""
   />
   ```
   To:
   ```tsx
   <DataSourceField
     value={varConfig.dataSource || ''}
     onChange={(ds) => onChange({ ...varConfig, dataSource: ds })}
     datasourceVariables={datasourceVariables}
     showNotebookOption={true}
     className=""
   />
   ```

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx` | Add `showNotebookOption={true}` prop to `DataSourceField` |

## Testing Strategy

1. `cd analytics-web-app && yarn lint && yarn type-check`
2. `cd analytics-web-app && yarn test`
3. Manual: create a notebook with a query cell producing a table, then add a dropdown variable cell — verify "Notebook (local)" appears in the datasource selector and that selecting it allows SQL queries against upstream cell results
