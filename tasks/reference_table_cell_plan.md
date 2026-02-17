# Reference Table Cell Plan

Issue: [#824](https://github.com/madesroches/micromegas/issues/824)

## Mockup

See [reference_table_cell_mockup.html](reference_table_cell_mockup.html) for a visual mockup showing rendered, editing, downstream usage, and error states.

## Overview

Add a new notebook cell type called **reference table** that allows users to provide hard-coded CSV data directly in the cell configuration. When executed, the CSV is parsed into an Arrow table, registered in the WASM DataFusion context, and made queryable by downstream cells via SQL.

This enables users to join or enrich query results with small reference datasets (lookup tables, constants, mappings) without needing to store them in the data lake.

## Current State

The notebook has 8 cell types registered in a static `CELL_TYPE_METADATA` map in `cell-registry.ts`:

- **Query-based**: `table`, `chart`, `log`, `propertytimeline`, `swimlane` — execute SQL via `runQuery()`
- **Data-producing non-SQL**: `variable` — executes SQL or expressions, produces variable values
- **Display-only**: `markdown`, `perfettoexport` — no query execution

Each cell type exports a metadata object (`CellTypeMetadata`) with renderer, editor, execute function, and configuration factory.

The execution context (`CellExecutionContext`) currently exposes:
- `variables` — upstream variable values
- `timeRange` — ISO time range
- `runQuery(sql)` — runs SQL and registers the result in WASM under the cell name

The WASM engine's `register_table(name, ipc_bytes)` is called internally by `runQuery`, but there is no way for a cell to register raw (non-SQL) data directly through the execution context.

## Design

### New Config Type

```typescript
// in notebook-types.ts
export interface ReferenceTableCellConfig extends CellConfigBase {
  type: 'referencetable'
  csv: string  // CSV text with headers
  options?: Record<string, unknown>  // Sort/hidden-column state (same pattern as QueryCellConfig)
}
```

### Execution Context Extension

Add an optional `registerTable` method to `CellExecutionContext`:

```typescript
// in cell-registry.ts
export interface CellExecutionContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>
  registerTable?: (ipcBytes: Uint8Array) => void  // Register raw IPC data under cell name
}
```

In `useCellExecution.ts`, populate it when the WASM engine is available:

```typescript
registerTable: engine
  ? (ipcBytes: Uint8Array) => { engine.register_table(cell.name, ipcBytes) }
  : undefined,
```

This is a minimal, backward-compatible change — existing cells ignore the new field.

### CSV-to-Arrow Conversion

A utility function `csvToArrowIPC(csvText: string)` that:

1. Parses CSV text using `csvParse` from `d3-dsv` (RFC 4180-compliant, handles quoting, escapes, CRLF)
2. Infers column types: if all non-empty values in a column parse as numbers, use `Float64`; otherwise use `Utf8`
3. Builds an Arrow table using `tableFromArrays()` from `apache-arrow`
4. Serializes to IPC stream bytes using `tableToIPC(table, 'stream')`
5. Returns both the IPC bytes and the Arrow `Table` (for display)

This utility lives in a new file `csv-to-arrow.ts` alongside the cell, or in `lib/` if reuse is likely. Given the focused scope, placing it in the cells directory is preferable.

### Execution Flow

```
User writes CSV in editor
        │
        ▼
  execute(config, context)
        │
        ▼
  csvToArrowIPC(config.csv)
        │
        ├─► { table, ipcBytes }
        │
        ▼
  context.registerTable?.(ipcBytes)   ← registers in WASM under cell name
        │
        ▼
  return { data: table }              ← displayed in renderer
```

### Renderer

Reuses the existing `TableBody` and `SortHeader` components from `table-utils.tsx` to display the parsed data in the notebook body — this serves as both the cell output and the live preview of the CSV. When the user edits CSV in the editor panel and re-executes, the table in the notebook body updates immediately.

Uses the `useColumnManagement` hook from `table-utils.tsx` to support column sorting and hiding, matching the pattern from `TableCell.tsx`. Sort and hidden-column state is persisted in `config.options` via `onOptionsChange`.

### Editor

- Plain `<textarea>` for editing CSV content (no need for code editor features like line numbers or syntax highlighting)
- Displays validation errors (e.g., "CSV must have at least a header row")
- No inline preview needed — the cell's rendered table in the notebook body serves as the preview

## Implementation Steps

### Step 1: Add type definitions

**`notebook-types.ts`**:
- Add `'referencetable'` to the `CellType` union
- Add `ReferenceTableCellConfig` interface with `csv: string`
- Add `ReferenceTableCellConfig` to the `CellConfig` union type

### Step 2: Extend execution context

**`cell-registry.ts`**:
- Add `registerTable?: (ipcBytes: Uint8Array) => void` to `CellExecutionContext`

**`useCellExecution.ts`**:
- Populate `registerTable` in the context object when `engine` is non-null
- Add `engine.deregister_table(cellName)` to `removeCellState` so deleted cells are cleaned up from WASM
- Add `engine.deregister_table(oldName)` to `migrateCellState` so renamed cells don't leave stale registrations (the next execution will re-register under the new name)

### Step 3: Create CSV-to-Arrow utility

**`cells/csv-to-arrow.ts`** (new file):
- Uses `csvParse` from `d3-dsv` for RFC 4180-compliant CSV parsing
- `csvToArrowIPC(csvText: string): { table: Table; ipcBytes: Uint8Array }` — full pipeline
- Validate: at least one header, at least one data row
- Type inference: try `Number()` on each column, fall back to string

### Step 4: Create ReferenceTableCell

**`cells/ReferenceTableCell.tsx`** (new file):
- **Renderer**: Displays the Arrow table using `SortHeader` + `TableBody` from `table-utils.tsx`, with `useColumnManagement` hook for sort/hide state persisted in `config.options`
- **Editor**: Plain `<textarea>` for CSV text input, validation error display
- **Metadata** (`referenceTableMetadata`):
  - `label: 'Reference Table'`
  - `icon: 'R'`
  - `description: 'Inline CSV data as a queryable table'`
  - `showTypeBadge: true`
  - `defaultHeight: 200`
  - `canBlockDownstream: true`
  - `createDefaultConfig`: returns `{ type: 'referencetable', csv: 'column1,column2\nvalue1,value2' }`
  - `execute`: parse CSV → register in WASM → return data
  - `getRendererProps`: extracts `data`, `status`, and `options`

### Step 5: Register in cell registry

**`cell-registry.ts`**:
- Import `referenceTableMetadata` from `./cells/ReferenceTableCell`
- Add `referencetable: referenceTableMetadata` to `CELL_TYPE_METADATA`

### Step 6: Update QueryCellConfig type constraint

**`notebook-types.ts`**:
- `QueryCellConfig.type` currently lists all SQL-based types. No change needed since `referencetable` is not SQL-based.

## Files to Modify

| File | Action |
|------|--------|
| `analytics-web-app/src/lib/screen-renderers/notebook-types.ts` | Add type to union, add config interface |
| `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` | Add `registerTable` to context, register new cell |
| `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts` | Populate `registerTable` in context |
| `analytics-web-app/src/lib/screen-renderers/cells/csv-to-arrow.ts` | **New** — CSV parsing and Arrow conversion |
| `analytics-web-app/src/lib/screen-renderers/cells/ReferenceTableCell.tsx` | **New** — cell renderer, editor, metadata |

## Trade-offs

### CSV parsing: d3-dsv vs. custom
**Chosen**: `d3-dsv` library (~2kB min+gz). Provides a battle-tested RFC 4180 parser that correctly handles quoting, escaped quotes, CRLF line endings, and fields containing newlines — edge cases that are deceptively tricky to get right in a hand-rolled parser. The size cost is negligible.

### Execution context vs. direct engine access
**Chosen**: Add `registerTable` to `CellExecutionContext`. This keeps the clean separation between cells and the WASM engine — cells never directly access the engine. The alternative (passing the engine to cell execute functions) would break the abstraction.

### Type inference
**Chosen**: Auto-detect numbers vs. strings per column. This covers the most common case (numeric codes, measurements). If all values in a column parse as numbers, use Float64; otherwise use Utf8. Users who need specific types can cast in downstream SQL. An alternative would be explicit type annotations in the CSV headers (e.g., `code:int,label:string`) but this adds complexity without clear benefit.

## Testing Strategy

### Unit tests for CSV parsing (`csv-to-arrow.test.ts`)
- Basic CSV: headers + data rows → correct Arrow schema and values
- Numeric detection: column with all numbers → Float64 type
- Mixed values: column with mix of numbers and strings → Utf8 type
- Quoted fields: handle commas inside quotes, escaped quotes
- Edge cases: empty cells, single column, trailing newlines
- Error cases: empty string, headers only, mismatched column count

### Unit tests for cell execution
- Execute with WASM engine: verify `registerTable` called with IPC bytes, data returned
- Execute without engine: verify data still returned (just no registration)
- Invalid CSV: verify error state

### Manual testing
- Create a reference table cell with sample CSV
- Verify data displays correctly in the rendered table
- Create a downstream SQL cell that queries the reference table by name
- Verify JOIN between a remote query result and the reference table
- Test editing CSV and re-executing: verify downstream cells update

## Open Questions

None.
