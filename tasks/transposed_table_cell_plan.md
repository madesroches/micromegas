# Transposed Table Notebook Cell Plan

## Overview

Add a new "Transposed Table" cell type to notebooks that displays SQL query results in a transposed layout — column names become row headers on the left, and each data row becomes a value column. This is the notebook equivalent of the Info Cards on the process page (`ProcessPage.tsx:486-507`), but generalized to support multiple data rows displayed as side-by-side columns.

## Current State

The process page uses `InfoRow` components to display key-value pairs (label left, value right) in styled cards. This layout doesn't exist as a notebook cell type. The closest is the regular `Table` cell, which displays data in standard row/column orientation.

The notebook system has 10 cell types registered in `cell-registry.ts`. Adding a new type requires:
1. A config type in `notebook-types.ts`
2. A cell file with renderer, editor, and metadata
3. Registration in `cell-registry.ts`

## Design

### Data Model

A transposed table takes SQL results and pivots the display:

**SQL returns** (e.g., 2 rows × 3 columns):
| exe | computer | username |
|-----|----------|----------|
| app1 | host-a | alice |
| app2 | host-b | bob |

**Transposed display** (3 rows × 2 value columns):
| | | |
|----------|--------|-------|
| exe | app1 | app2 |
| computer | host-a | host-b |
| username | alice | bob |

With a single row result, this collapses to the Info Card layout (label → value).

### Config

New cell type `'transposed'` as a `QueryCellConfig` variant — it executes SQL like table/chart cells.

No additional config fields beyond what `QueryCellConfig` provides. The `options` bag can hold display preferences if needed later (e.g., column header labels).

### Renderer

- Left column: field names (from Arrow schema), styled as muted labels
- Right columns: one per data row, values displayed as primary text
- Rows separated by bottom borders (matching InfoRow style)
- Scrollable if content overflows the cell height
- Monospace for values that look like IDs/timestamps (optional, can be deferred)

### Editor

Standard SQL editor — same pattern as TableCell editor: `SyntaxEditor` for SQL, `AvailableVariablesPanel`, `DocumentationLink`.

## Implementation Steps

1. **Add type to `notebook-types.ts`**
   - Add `'transposed'` to the `CellType` union
   - Add `'transposed'` to the `QueryCellConfig.type` union

2. **Create `cells/TransposedTableCell.tsx`**
   - Renderer: iterates over Arrow schema fields as rows, renders each data row as a value column
   - Editor: SQL editor (reuse `SyntaxEditor`, `AvailableVariablesPanel`)
   - Metadata export: `transposedTableMetadata`

3. **Register in `cell-registry.ts`**
   - Import `transposedTableMetadata`
   - Add `transposed: transposedTableMetadata` to `CELL_TYPE_METADATA`

4. **Update `createDefaultCell` in `cell-registry.ts`**
   - The `transposed` type uses SQL, so it should get the default data source (no special exclusion needed — it already falls through)

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/notebook-types.ts` — add `'transposed'` to type unions
- `analytics-web-app/src/lib/screen-renderers/cells/TransposedTableCell.tsx` — new file
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` — register new cell type

## Testing Strategy

1. Build: `cd analytics-web-app && yarn build` (type-check + bundle)
2. Manual: create a notebook, add a transposed table cell, run a query that returns 1 row → verify info-card layout
3. Manual: run a query returning multiple rows → verify multi-column layout
4. Manual: run a query returning 0 rows → verify empty state

## Decisions

- **Name**: "Transposed", type key `transposed`
- **No column headers for multi-row**: value columns have no headers, consistent with tables not showing row indices
