# Refactor `CellState.data` from `Table | null` to `Table[]`

## Goal

Replace the split `data: Table | null` + `additionalData?: Table[]` with a single uniform `data: Table[]`. Empty array `[]` replaces `null` as the "no data" sentinel. This simplifies the cell system and removes the arbitrary primary/additional split introduced by multi-query charts.

## Status: COMPLETE

All steps done. Type-check, lint, and tests (716/716) pass.

## Steps completed

1. **Type definitions** — `notebook-types.ts`, `cell-registry.ts`: `data: Table[]`, `additionalData` removed
2. **Central normalization** — `useCellExecution.ts`: all `data: null` → `data: []` (6 sites)
3. **Cell execute() + renderers** — all cells wrap single tables in arrays, renderers use `data[0]`
4. **ChartCell** — flat `data: tables` array, `data.length > 1` for multi-series, `additionalData` eliminated
5. **NotebookRenderer** — default state `data: []`, aggregate stats across array, `data[0]?.schema.fields`
6. **Tests** — all assertions and mock props updated, test utility mock updated

## Files modified

| File | Change |
|------|--------|
| `src/lib/screen-renderers/notebook-types.ts` | `data: Table[]`, removed `additionalData` |
| `src/lib/screen-renderers/cell-registry.ts` | `data: Table[]`, removed `additionalData` |
| `src/lib/screen-renderers/useCellExecution.ts` | 6 sentinel sites `null` → `[]` |
| `src/lib/screen-renderers/NotebookRenderer.tsx` | Aggregate stats, `data[0]` for columns |
| `src/lib/screen-renderers/cells/ChartCell.tsx` | Flat array, drop `additionalData` |
| `src/lib/screen-renderers/cells/TableCell.tsx` | `const table = data[0]`, wrap in execute |
| `src/lib/screen-renderers/cells/LogCell.tsx` | Same pattern |
| `src/lib/screen-renderers/cells/PropertyTimelineCell.tsx` | Same pattern |
| `src/lib/screen-renderers/cells/SwimlaneCell.tsx` | Same pattern |
| `src/lib/screen-renderers/cells/ReferenceTableCell.tsx` | Same pattern |
| `src/lib/screen-renderers/cells/VariableCell.tsx` | `data: []` / `data: [result]` in execute |
| `__tests__/useCellExecution.test.ts` | `data: null` → `data: []`, `.not.toBeNull()` → `.length > 0` |
| `cells/__tests__/VariableCell.test.tsx` | All mock props and CellState |
| `cells/__tests__/PerfettoExportCell.test.tsx` | Mock props and CellState |
| `cells/__tests__/MarkdownCell.test.tsx` | Mock props |
| `__test-utils__/cell-registry-mock.ts` | Execute stubs wrap results |
