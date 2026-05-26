# OverrideCell Memo Fix Plan

## Issue Reference
- [#1092](https://github.com/madesroches/micromegas/issues/1092) — OverrideCell: useMemo deps include fresh-per-render objects; evaluateTemplate runs every render

## Overview

`OverrideCell`'s `useMemo` around `evaluateTemplate` lists object references
(`row`, `variables`, `cellResults`, `cellSelections`) that are reconstructed
on every parent render. The memo therefore never hits — `evaluateTemplate`
runs on every render of every override cell in the table. Fix by hashing the
template's actual inputs to a stable string key and memoizing on that key,
mirroring the existing `useColumnWarnings` content-hash pattern
(`warning-reporter.tsx:37-41`).

## Current State

### The misbehaving memo — `table-utils.tsx:178-187`

```ts
const { text: expanded, warnings } = useMemo(() => {
  return evaluateTemplate(format, {
    variables,
    timeRange,
    cellResults,
    cellSelections,
    row,
    columnTypes,
  })
}, [format, variables, timeRange, cellResults, cellSelections, row, columnTypes])
```

Why each dep behaves:

| Dep | Source | Stability |
|---|---|---|
| `format` | string prop | ✓ primitive, stable |
| `columnTypes` | `useMemo` at `table-utils.tsx:170-176` | ✓ stable while columns are |
| `row` | `data.get(rowIdx)` inside `TableBody`'s `Array.from` loop (`table-utils.tsx:469-470`) | ✗ fresh object literal per render |
| `variables`, `timeRange`, `cellSelections`, `cellResults` | Threaded down from `CellRendererProps` via `TableCell` / `TableRenderer` / `TransposedTableCell` without parent-side memoization (`TableCell.tsx:171-174`, etc.) | ✗ typically fresh references per parent render |

Because `row` alone changes identity every render, the memo's cache key is
invalidated on every render — `evaluateTemplate` runs unconditionally. The
other unstable deps compound the problem but `row` already guarantees it.

### Precedent for the fix — `warning-reporter.tsx:37-58`

`useColumnWarnings` faced the same "fresh reference, identical content"
problem with the `overridesSource` array and solved it by content-hashing
via `JSON.stringify`. Comment at line 38-40 notes this is acceptable because
the input is "a small array of `{ column, format }` objects." `OverrideCell`
needs the same technique, but adapted for inputs that can contain Arrow
values (including BigInts), so plain `JSON.stringify` will throw.

### Template-input extraction is already available — `table-utils.tsx:46-63`

`extractMacroColumns(template)` returns the column names referenced by
`$row.x` / `$row["x"]`. The same extraction approach can be reused (or
extended) to know which subset of `row` and `variables` actually affects
the result — letting the hash skip irrelevant columns/variables entirely.

### Consumers of `OverrideCell`

- `TableBody` (`table-utils.tsx:501`) — used by `TableCell`, `TableRenderer`, `ReferenceTableCell`.
- `TransposedTableCell.tsx:123` — direct render per cell.

All three sites pass the same set of context props.

## Design

### Approach: BigInt-safe content hash, scoped to template-referenced inputs

Keep `OverrideCell`'s responsibilities intact; replace the broken memo with
one keyed on a stable hash string built from:

1. `format` (primitive, used as-is).
2. The row values at columns **referenced by the template**, only.
3. The variables, cell selections, and cell results actually referenced.
4. `timeRange.begin` + `timeRange.end` (two strings).

Computing the referenced-input subset avoids hashing the entire `row` /
`variables` / `cellResults` payloads, which can be large in the data-lake
context (Arrow Tables with many columns).

#### 1. Referenced-name extraction — one helper, all shapes

`extractMacroColumns` already returns `$row.x` / `$row["x"]` column names
and stays as-is (it's a public export used by `validateFormatMacros`).
For the hash builder, add one helper next to it (exported alongside
`extractMacroColumns` so unit tests can call it directly) that walks the
template in a single regex pass and returns everything else:

```ts
interface MacroRefs {
  /** `$row.x` / `$row["x"]` column names. */
  rowCols: string[]
  /** `$name` and `$name.col` heads (excludes `$row`, `$from`, `$to`). */
  variableNames: string[]
  /** `$cell.selected.col` — map of cell name → columns used. */
  cellSelections: Map<string, string[]>
  /** `$cell[N].col` — map of cell name → { columns referenced, row indices
   *  referenced }. Indices are retained so the hash only reads the rows
   *  the template actually consults (upstream cell-result tables can be
   *  much larger than the current table's paginated view). */
  cellResults: Map<string, { cols: string[]; indices: number[] }>
}

function extractMacroRefs(template: string): MacroRefs
```

The function-call paths (`format_value(...)`) feed their arguments through
`tryParseMacro`, so the macros inside the call site are still `$row.x` /
`$cell.selected.x` / `$variable` etc. — no extra extraction logic needed
for `(...)` arguments.

#### 2. BigInt-safe stringify

```ts
function stableStringify(value: unknown): string {
  return JSON.stringify(value, (_k, v) => {
    if (typeof v === 'bigint') return `__bigint:${v.toString()}`
    if (v instanceof Uint8Array) return `__bytes:${v.length}` // length-only is enough for keying
    return v
  })
}
```

Justification: Arrow time/duration values arrive as BigInt; the existing
SQL-targeted `formatArrowValue` (`macro-substitution.ts`) already understands
how to render them, but for cache-keying purposes the raw `toString()` is
sufficient and stable. Length-only for binary cells avoids hashing megabyte
blobs.

#### 3. Hash builder — new helper in `table-utils.tsx`

```ts
/** Build a stable cache key from only the inputs that `evaluateTemplate(format, ctx)` consults. */
function buildEvaluateKey(
  format: string,
  ctx: EvaluateTemplateCtx,
  refs: MacroRefs,
): string {
  // Project each input down to referenced fields before stringifying.
  // Row values AND their column types both feed the hash: `evaluateTemplate`
  // emits naked `$row.col` macros via `formatArrowValue(value, dataType)`
  // (template-evaluator.ts:367), so a type change (e.g., column re-typed
  // from string to Timestamp) flips the rendering from raw to RFC3339 even
  // when the underlying value is unchanged.
  const rowSlice: Record<string, unknown> = {}
  const rowColTypes: Record<string, string | null> = {}
  if (ctx.row) {
    for (const c of refs.rowCols) {
      rowSlice[c] = ctx.row[c]
      rowColTypes[c] = ctx.columnTypes?.get(c)?.toString() ?? null
    }
  }

  const varSlice: Record<string, unknown> = {}
  for (const n of refs.variableNames) varSlice[n] = ctx.variables[n]

  // Cell selections render via `formatArrowValue(value, dataType)` where the
  // DataType comes from the upstream cell-result table's schema
  // (template-evaluator.ts:160), NOT from `columnTypes`. Hash both the value
  // and that schema type — same reasoning as `rowColTypes` above: an upstream
  // column re-typed without its value changing must still flip the key.
  const selSlice: Record<string, Record<string, unknown>> = {}
  const selColTypes: Record<string, Record<string, string | null>> = {}
  for (const [cell, cols] of refs.cellSelections) {
    const sel = ctx.cellSelections[cell]
    if (sel) {
      const s: Record<string, unknown> = {}
      const t: Record<string, string | null> = {}
      const table = ctx.cellResults[cell]
      for (const c of cols) {
        s[c] = sel[c]
        t[c] = table?.schema.fields.find((f) => f.name === c)?.type?.toString() ?? null
      }
      selSlice[cell] = s
      selColTypes[cell] = t
    }
  }

  // For cellResults: project only the rows the template actually references
  // (the `N` from `$cell[N].col`) down to the referenced columns. Upstream
  // cell-result tables can be much larger than the current table's
  // pagination window — iterating every row would re-hash megabytes of
  // unrelated data per override cell per render.
  const resSlice: Record<string, unknown> = {}
  const resColTypes: Record<string, Record<string, string | null>> = {}
  for (const [cell, { cols, indices }] of refs.cellResults) {
    const t = ctx.cellResults[cell]
    if (t) {
      const rows: Record<string, unknown>[] = []
      for (const i of indices) {
        if (i >= t.numRows) continue
        const r = t.get(i)
        if (!r) continue
        const proj: Record<string, unknown> = {}
        for (const c of cols) proj[c] = r[c]
        rows.push(proj)
      }
      resSlice[cell] = rows
      // Same as selColTypes: `$cell[N].col` renders via
      // `formatArrowValue(value, dataType)` with the type from the table
      // schema (template-evaluator.ts:113), so a re-type must flip the key.
      const tt: Record<string, string | null> = {}
      for (const c of cols) {
        tt[c] = t.schema.fields.find((f) => f.name === c)?.type?.toString() ?? null
      }
      resColTypes[cell] = tt
    }
  }

  return stableStringify({
    format,
    timeRange: ctx.timeRange ? [ctx.timeRange.begin, ctx.timeRange.end] : null,
    row: rowSlice,
    rowColTypes,
    variables: varSlice,
    cellSelections: selSlice,
    selColTypes,
    cellResults: resSlice,
    resColTypes,
  })
}
```

The `cellResults` map returned by `extractMacroRefs` keys per-cell entries
by cell name (the literal `$ident[N].col` capture), and each entry retains
both the referenced columns and the referenced row indices. The hash only
projects those specific rows — matching what `evaluateTemplate` actually
reads — so a large upstream cell-result table doesn't get scanned on every
render.

#### 4. The fixed `OverrideCell` memo

```ts
const refs = useMemo(() => extractMacroRefs(format), [format])

const cacheKey = buildEvaluateKey(
  format,
  { variables, timeRange, cellResults, cellSelections, row, columnTypes },
  refs,
)

const { text: expanded, warnings } = useMemo(
  () => evaluateTemplate(format, { variables, timeRange, cellResults, cellSelections, row, columnTypes }),
  // eslint-disable-next-line react-hooks/exhaustive-deps
  [cacheKey],
)
```

`cacheKey` is a string; React's `useMemo` compares strings by value, so a
re-render with semantically identical inputs reuses the prior result. The
`exhaustive-deps` suppression mirrors the pattern in `warning-reporter.tsx`
and the existing `[warningKey, columnName, reporter]` effect at
`table-utils.tsx:191-199`.

### Why not the alternatives

- **Remove the memo entirely.** Satisfies the acceptance criteria but
  pre-`format_value` overrides did real string work per call, and post-
  `format_value` the call can do adaptive-unit lookups. The fix should be
  the cheap one that actually preserves the original intent.
- **Move evaluation up to `TableBody` and pass `{ text, warnings }` down as
  a precomputed prop.** Cleaner separation but a larger refactor touching
  three call sites (`TableCell`, `TableRenderer`, `ReferenceTableCell`,
  plus `TransposedTableCell` which uses `OverrideCell` directly without
  going through `TableBody`). Higher blast radius for an "insignificant
  until row counts grow" cleanup. Worth revisiting if profile data later
  shows the per-row hash itself is the bottleneck.
- **Wrap upstream parents in `useMemo` to stabilize `variables` /
  `cellSelections` / `cellResults`.** Pushes the fix far away from the
  problem and is fragile — any future contributor adding a non-memoized
  prop above the table silently regresses behavior. The hash makes the
  component robust regardless of parent discipline (same argument used in
  the `useColumnWarnings` doc comment at line 14-18).

## Implementation Steps

1. **Add `extractMacroRefs` and `MacroRefs`** in
   `analytics-web-app/src/lib/screen-renderers/table-utils.tsx`, near
   `extractMacroColumns` (line 46). Single regex pass returning the four
   reference categories the hash builder needs. Co-locate so future
   template additions update one file. `extractMacroColumns` itself is
   left untouched (still public, still used by `validateFormatMacros`).

2. **Add `stableStringify` and `buildEvaluateKey`** helpers in the same
   file, scoped to the override-cell section. Export `buildEvaluateKey`
   (and `extractMacroRefs` from Step 1) so the unit tests can call them
   directly; `stableStringify` stays private and is covered transitively
   through `buildEvaluateKey` tests.

3. **Rewrite `OverrideCell`'s memo** (lines 178-187) per the snippet in
   §4 above. Wrap the `refs` computation in `useMemo` so extraction only
   re-runs when the format string changes.

4. **Add unit tests** in
   `analytics-web-app/src/lib/screen-renderers/__tests__/table-utils.test.tsx`.
   `extractMacroRefs` and `buildEvaluateKey` need to be exported for
   testing; `stableStringify` can stay private (covered transitively):
   - `extractMacroRefs` — one `describe` block with separate `it`s per
     returned field: `rowCols` (covered by `extractMacroColumns` already,
     so a sanity check is enough), `variableNames` (`$x`, `$x.col`,
     ignores `$row`/`$from`/`$to`), `cellSelections`
     (`$cell.selected.col`), `cellResults` (`$cell[0].col` and
     `$cell[2].col` on the same cell produce one entry with cols
     deduped and both indices retained).
   - `buildEvaluateKey` — stable across two calls with structurally equal
     inputs that are fresh references each time; changes when a referenced
     column value changes; ignores unreferenced columns; handles BigInt
     row values without throwing; **changes when the DataType of a
     referenced row column changes** (e.g., the same epoch-bigint value
     under a `Timestamp` type vs. a bare numeric type produces different
     keys, mirroring the different rendered output); **also changes when a
     referenced `$cell[N].col` or `$cell.selected.col` column is re-typed
     in the upstream cell-result schema** while its value is unchanged
     (same reasoning, since those paths format via the table schema's
     DataType, not `columnTypes`).
   - `OverrideCell` — render twice with new identity but equal content,
     assert `evaluateTemplate` is called once. **Do not use
     `jest.spyOn(notebookUtils, 'evaluateTemplate')`** — this repo's Jest
     runs in ESM mode (`useESM: true`, `extensionsToTreatAsEsm: ['.ts',
     '.tsx']` in `jest.config`), so module-namespace exports are read-only
     and `spyOn` throws "Cannot redefine property." The codebase has zero
     `jest.spyOn` uses and mocks exclusively via `jest.mock(...)`
     factories. Use a counting mock that wraps the real implementation:
     ```ts
     jest.mock('../notebook-utils', () => {
       const actual = jest.requireActual('../notebook-utils')
       return { ...actual, evaluateTemplate: jest.fn(actual.evaluateTemplate) }
     })
     ```
     then assert `(evaluateTemplate as jest.Mock).mock.calls.length`.
     `evaluateTemplate` is re-exported from `notebook-utils` (which is what
     `table-utils.tsx` imports at line 23), so mock that module, not
     `template-evaluator`. Alternatively, drop the call-count assertion
     entirely and rely on the `buildEvaluateKey` stability tests (a stable
     key guarantees the memo holds) plus a render-output test.

5. **Run lint + tests + type-check**:
   - `yarn lint`
   - `yarn type-check`
   - `yarn test`

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/table-utils.tsx` — extractors, hash helpers, `OverrideCell` memo.
- `analytics-web-app/src/lib/screen-renderers/__tests__/table-utils.test.tsx` — new tests.

No changes to `template-evaluator.ts`, `macro-substitution.ts`,
`notebook-utils.ts`, or any consumer of `OverrideCell` / `TableBody`.

## Trade-offs

- **Hash cost vs. evaluation cost.** Per-render we now build a small JSON
  string for each override cell instead of running `evaluateTemplate`. For
  typical templates referencing a handful of columns, hashing is the
  cheaper operation (no Arrow value coercion, no Markdown-bound string
  building, no warning aggregation). Templates that reference many
  columns or have large `$cell[N].col` payloads narrow that gap, but the
  break-even is still well above the practical row counts and template
  shapes we see in practice — the table cell paginates via
  `usePagination`, capping visible rows at `DEFAULT_PAGE_SIZE`.
- **Hash collisions are non-recoverable.** A collision returns stale
  output. `stableStringify` is deterministic JSON, so collisions require
  semantically equal inputs by construction — the failure mode is
  benign.
- **Bytes columns: length-only fingerprint.** A cell whose binary payload
  changes content without changing length would incorrectly cache-hit.
  Acceptable because binary cells are not a sensible template input
  today (`formatCell` renders them as an ASCII preview, and overrides
  rarely target them). If this becomes a real concern, swap to a
  rolling fingerprint.

## Testing Strategy

- **Unit tests** as listed in Implementation Step 4.
- **Manual smoke test** in the dev server (`yarn dev`):
  1. Open a notebook with a `table` cell whose query produces multiple
     rows.
  2. Add an override on a column with a `format` like
     `**$row.name**: format_value($row.value, 'bytes')`.
  3. Confirm rows render correctly.
  4. Add `$variable` and `$cell.selected.col` references and confirm
     they re-evaluate when the upstream variable/selection changes,
     and *don't* re-evaluate when an unrelated re-render happens
     (e.g. selecting a different row in the same table).
- **Regression check** for the warning surface: change an override's
  `format` to something that emits warnings (e.g. `format_value($row.x,
  'bytes')` where `x` is non-numeric) and confirm the column-header
  warning icon still appears. The `warnings` array is part of
  `evaluateTemplate`'s return; the existing `warningKey`-gated effect at
  `table-utils.tsx:191-199` should remain unchanged in behavior.

## Documentation

No user-facing documentation changes — this is a pure rendering-performance
fix. Internal comments inside `OverrideCell` should be updated to explain
why the memo is keyed on a content hash (mirroring the explanation in
`warning-reporter.tsx:37-50`), so the next person to touch this code
doesn't undo the fix by replacing `[cacheKey]` with the "obvious" prop
list.

