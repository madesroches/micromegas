# Map Detail Template: Raw Row + Column Types Plan

Addresses [issue #1091](https://github.com/madesroches/micromegas/issues/1091).

## Overview

The Map cell's event-detail panel resolves `$column` macros by **eagerly
stringifying every column** of the selected row (`materializeRow`) and then
spreading that string map into the template's `variables` argument. This works
for plain text and timestamps today, but it forecloses the richer behavior the
table-override path already has: because values reach the template pre-stringified,
`format_value($size, 'bytes')` in a Map template receives a string instead of the
raw number/BigInt, losing precision and adaptive formatting.

This plan routes the selected row through the evaluator's existing **raw `row` +
`columnTypes`** channel (the same one `OverrideCell` uses) instead of the
variables map. Bare `$column` macros become first-class row references that carry
their Arrow `DataType`, so they format through `formatArrowValue` (RFC3339 for
timestamps) and feed `format_value()` with full-precision raw values.

## Current State

### Selected-row materialization (eager stringification)

`materializeRow` (`analytics-web-app/src/components/map/overlay.ts:565-575`)
converts a whole Arrow row to `Row = Record<string, string>`
(`overlay.ts:16`), calling `formatArrowValue(value, field.type)` per column:

```ts
export function materializeRow(table: Table, rowIndex: number): Row {
  const row: Row = {}
  for (const field of table.schema.fields) {
    const col = table.getChild(field.name)
    if (!col) continue
    const value = col.get(rowIndex)
    if (value === null || value === undefined) continue
    row[field.name] = formatArrowValue(value, field.type)  // <-- stringified here
  }
  return row
}
```

Two consumers in `MapCell.tsx`:
- `selectedRow` (`MapCell.tsx:326-331`) → passed to `EventDetailPanel`.
- `onSelectionChange` (`MapCell.tsx:320`) → feeds `cellSelections[mapCellName]`
  for cross-cell `$mapcell.selected.col` references.

### EventDetailPanel merge

`EventDetailPanel` (`analytics-web-app/src/components/map/EventDetailPanel.tsx:56-66`)
spreads the (already-stringified) row into `variables`:

```ts
const mergedVars: Record<string, VariableValue> = { ...variables, ...row }
return evaluateTemplate(template, { variables: mergedVars, timeRange, cellResults, cellSelections })
```

`row` columns win name collisions against notebook variables because the row is
spread last (documented at `cell-types.md:285`, tested at
`EventDetailPanel.test.tsx:58-65`).

> **Note on the issue text:** #1091 describes `row` as `Record<string, unknown>`
> spread into `Record<string, VariableValue>` and renders as `[object Object]`.
> Since #1057 narrowed `Row` to `Record<string, string>` and made
> `materializeRow` stringify eagerly, the literal type-assertion-lie / RFC3339
> bug no longer reproduces — acceptance criterion 1 already passes at runtime.
> What remains is the **architectural cleanup** the issue asks for: drop the
> merge-into-variables shoehorn, route `$col` through the raw `row` + `columnTypes`
> channel like the table-override path. The concrete payoff is precision-preserving
> `format_value($col, unit)` in Map templates, which the current eager
> stringification defeats.

### The evaluator already supports raw row + column types

`EvaluateTemplateCtx` (`template-evaluator.ts:23-35`) already carries `row` and
`columnTypes`, but they are consulted **only** for the dotted/bracketed forms
`$row.col` and `$row["col"]` (`template-evaluator.ts:120-135`, `168-177`).
`OverrideCell` (`table-utils.tsx:361-405`) builds `columnTypes` from the table
schema and passes the raw `row` slice; naked `$row.col` macros render through
`formatArrowValue(value, dataType)` (`template-evaluator.ts:367`), and
`format_value($row.col, 'bytes')` receives the raw value.

The gap: **bare `$column`** (branch 6, `$variable`, `template-evaluator.ts:192-201`)
resolves only against `ctx.variables` — it never consults `ctx.row`. The Map cell
uses bare `$column`, the table-override path uses `$row.column`. These have
**different precedence rules for bare identifiers**:
- Map: bare `$name` → row column (wins over variable).
- Table override: bare `$name` → variable (columns are addressed via `$row.name`).

A single unconditional rule cannot serve both, so the evaluator must be told which
mode to use.

### Validation path (separate, unaffected)

`MapCell.tsx:744-751` validates the detail template with the legacy regex-based
`validateMacros`, synthesizing empty-string column vars so `$col` isn't flagged
"unknown". This is presence-only and independent of `evaluateTemplate`; it keeps
working unchanged because `$col` still validates against the synthetic var map.

## Design

### 1. Add an opt-in "bare columns resolve from row" mode to the evaluator

Add a flag to `EvaluateTemplateCtx`:

```ts
export interface EvaluateTemplateCtx {
  // ...existing fields...
  row?: Record<string, unknown>
  columnTypes?: Map<string, DataType>
  /** When true, a bare `$ident` resolves to `row[ident]` (with its column
   *  DataType) before falling back to a notebook variable. Map detail
   *  templates set this so `$col` means "the selected row's col"; the
   *  table-override path leaves it false and addresses columns via `$row.col`. */
  bareColumnsFromRow?: boolean
}
```

In `tryParseMacro`, branch 6 (`$variable`, currently `template-evaluator.ts:192-201`),
check the row **first** when the flag is set, so columns win collisions
(preserving Map semantics):

```ts
// 6. $variable  (or $column when bareColumnsFromRow)
const end = afterIdent
const source = text.slice(pos, end)
const shape = `$${ident}`

if (ctx.bareColumnsFromRow && ctx.row !== undefined) {
  const v = ctx.row[ident]
  if (v !== undefined && v !== null) {
    return { source, end, shape, value: v, dataType: ctx.columnTypes?.get(ident) }
  }
}

const variable = ctx.variables[ident]
let value: unknown
if (variable !== undefined) {
  value = typeof variable === 'string' ? variable : getVariableString(variable)
}
return { source, end, shape, value }
```

This is the only evaluator change. The function-call branch already routes macro
args through `tryParseMacro`, so `format_value($size, 'bytes')` in a Map template
will now receive the raw value with its `DataType` (and the existing time-type
guard at `template-evaluator.ts:246-249` applies for free).

`$variable.col` (branch 5) is intentionally left alone: Map rows are flat column
maps, so there's no `$col.sub` shape to resolve from a row.

### 2. Provide raw row + column types instead of stringified row

Replace the eager `materializeRow` with two raw accessors in `overlay.ts`:

```ts
/** Raw column values for one row (no stringification). Null/undefined skipped. */
export function rowValues(table: Table, rowIndex: number): Record<string, unknown> {
  const row: Record<string, unknown> = {}
  for (const field of table.schema.fields) {
    const v = table.getChild(field.name)?.get(rowIndex)
    if (v === null || v === undefined) continue
    row[field.name] = v
  }
  return row
}

/** Column-name → Arrow DataType, for RFC3339 / format_value resolution. */
export function columnTypeMap(table: Table): Map<string, DataType> {
  return new Map(table.schema.fields.map((f) => [f.name, f.type]))
}
```

`columnTypeMap` can be memoized on `overlay.table` in `MapCell` (it only depends on
the schema, not the selected row).

Delete `materializeRow` and narrow/remove the `Row = Record<string, string>` alias
(replace its prop usage with `Record<string, unknown>`). Confirm no other consumer
of `Row` survives (only `EventDetailPanel` + tests today).

### 3. Wire MapCell + EventDetailPanel

`MapCell.tsx`:
- Swap the `overlay` import (`MapCell.tsx:24`): replace `materializeRow` with
  `rowValues` and `columnTypeMap`.
- `selectedRow` → `rowValues(overlay.table, selectedRowIndex)` (raw).
- Add memoized `columnTypes = columnTypeMap(overlay.table)`.
- `onSelectionChange` (`MapCell.tsx:320`) → pass the **raw** `rowValues(...)` so
  cross-cell `$mapcell.selected.col` matches the raw-row convention table cells
  already use (the evaluator resolves its DataType from `ctx.cellResults`).
- Pass `columnTypes` into `<EventDetailPanel>`.

`EventDetailPanel.tsx`:
- Change prop type: `row: Record<string, unknown>`, add `columnTypes: Map<string, DataType>`.
- Drop the `mergedVars` spread; call the evaluator directly:

```ts
return evaluateTemplate(template, {
  variables,
  timeRange,
  cellResults,
  cellSelections,
  row,
  columnTypes,
  bareColumnsFromRow: true,
})
```

- Update the `useMemo` dependency list (`row`, `columnTypes` instead of the merged map).

## Implementation Steps

1. **Evaluator** (`template-evaluator.ts`): add `bareColumnsFromRow` to
   `EvaluateTemplateCtx`; resolve bare `$ident` from `ctx.row` first when set
   (branch 6).
2. **Overlay** (`overlay.ts`): add `rowValues` + `columnTypeMap`; remove
   `materializeRow` and the `Row` string alias.
3. **MapCell** (`cells/MapCell.tsx`): use `rowValues` for `selectedRow` and
   `onSelectionChange`; memoize `columnTypes`; pass it to `EventDetailPanel`.
4. **EventDetailPanel** (`EventDetailPanel.tsx`): new `row`/`columnTypes` props;
   delete the merge; pass `bareColumnsFromRow: true`.
5. **Tests**: update `EventDetailPanel.test.tsx` and `MapCell.test.tsx`
   (`materializeRow` block) per the testing strategy below. Note for
   `EventDetailPanel.test.tsx`: drop the `import type { Row } from '../overlay'`
   (`EventDetailPanel.test.tsx:4`), retype `buildRow`/`renderPanel` rows to
   `Record<string, unknown>`, and pass the now-required `columnTypes` prop in
   `renderPanel`.
6. **Docs**: extend the Map detail-template section with the `format_value`
   capability note.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/template-evaluator.ts`
- `analytics-web-app/src/components/map/overlay.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`
- `analytics-web-app/src/components/map/EventDetailPanel.tsx`
- `analytics-web-app/src/components/map/__tests__/EventDetailPanel.test.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/MapCell.test.tsx`
- `mkdocs/docs/web-app/notebooks/cell-types.md`

## Trade-offs

- **Gating flag vs. unconditional bare-column resolution.** Making bare `$col`
  resolve from `row` unconditionally would be DRY-er but silently changes
  table-override semantics: a bare `$name` that happens to match a column would
  start resolving to that column instead of a variable (or instead of staying
  literal). The `bareColumnsFromRow` flag confines the new behavior to the Map
  path that wants it, preserving table-override behavior exactly. Chosen for
  least surprise; it adds one boolean to a context object both paths already build.
- **Raw `onSelectionChange` value.** Switching the cross-cell selection from
  stringified to raw aligns the Map cell with table cells (which publish raw
  `table.get(i)` rows) and lets `$mapcell.selected.col` format via the consuming
  cell's schema lookup. Risk: a downstream template relying on the old
  pre-stringified shape — none exists today; the evaluator stringifies via
  `formatArrowValue` at emission regardless.
- **Removing `materializeRow` outright** vs. keeping it. It has no surviving
  consumer once both call sites move to raw accessors; leaving it would be dead
  code. Removed.

## Documentation

`mkdocs/docs/web-app/notebooks/cell-types.md`, "Detail template" section
(lines ~279-313): add that `$column` carries its Arrow type, so timestamp columns
render RFC3339 and numeric columns can be wrapped with `format_value`, e.g.
`format_value($bytes_sent, 'bytes')` — matching the table-override capability.

## Testing Strategy

- **Unit (evaluator)**: with `bareColumnsFromRow: true` and a `row`/`columnTypes`
  pair, assert (a) a Timestamp column renders RFC3339 from bare `$ts`,
  (b) `format_value($size, 'bytes')` formats the raw BigInt/number (precision
  preserved), (c) bare `$name` still prefers the row over a same-named variable,
  (d) with the flag **false** (table-override default), bare `$name` resolves the
  variable, unchanged.
- **EventDetailPanel.test.tsx**: keep existing string-row cases (raw strings flow
  through `formatArrowValue`→`String()` unchanged); add a case building an Arrow
  Table row with a Timestamp column to prove RFC3339 + a `format_value` case.
- **MapCell.test.tsx**: replace the `materializeRow` block with `rowValues` /
  `columnTypeMap` tests (raw values returned; null/undefined skipped; type map
  matches schema).
- **Full**: `yarn lint`, `yarn type-check`, `yarn test` from `analytics-web-app/`.

## Open Questions

- None blocking. The `bareColumnsFromRow` flag name is a suggestion — an
  alternative is to model it as a mode enum if a third resolution policy ever
  appears, but a boolean is sufficient for the two policies that exist today.
