# Template Function Calls — `format_value` Plan

## Issue Reference
- [#1086](https://github.com/madesroches/micromegas/issues/1086) — Support function-call expressions in template macro engine

## Overview

Add a tiny function-call expression layer to the template macro engine so values interpolated into Markdown templates (Map `detailTemplate`, Table `format` overrides) can be rendered with the same adaptive unit formatting the chart cell already uses. v1 surface is a single function — `format_value(value, unit)` — that reuses the chart's adaptive formatters. The evaluator walks the template once and resolves macro arguments to **raw JS/Arrow values** (not strings), so byte counts and large floats keep full precision.

Example payoff:

| Template | Today | After |
|---|---|---|
| `$metric_avg` (value `3678630912`, unit `bytes`) | `3678630912` | (unchanged unless wrapped) |
| `format_value($metric_avg, $metric.unit)` | n/a | `3.4 GB` |
| `format_value($cell.selected.bytes, 'bytes')` | n/a | `3.4 GB` |
| `format_value($total_seconds, 'seconds')` | n/a | `4.07 milliseconds` |

## Current State

### Macro engine — `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`

`substituteMacrosImpl` (line 346) walks the input and replaces matches in this order:
1. `$from` / `$to` — `notebook-utils.ts:357-358`
2. `$cell[N].column` — `notebook-utils.ts:362-371`
3. `$cell.selected.column` — `notebook-utils.ts:376-384`
4. `$variable.column` — `notebook-utils.ts:388-403`
5. `$variable` — `notebook-utils.ts:408-419`

Each replacement runs the matched raw value through `formatArrowValue` (line 279, which handles timestamps → RFC3339) and then through an `escape` callback. Two public entry points wrap the impl:

- `substituteMacros` (line 321) — uses `escapeSqlValue` (line 290): single-quote doubling for SQL safety.
- `substituteMacrosRaw` (line 336) — identity escape; used by Map overlay (`components/map/overlay.ts:495`) for non-SQL string interpolation.

### Markdown template callers

| Caller | Function | Notes |
|---|---|---|
| Map `detailTemplate` | `components/map/EventDetailPanel.tsx:59` calls `substituteMacros(template, mergedVars, …)` | Row columns are merged into `mergedVars` so `$x` resolves to the row's `x` column. Output is rendered as Markdown. |
| Table column `format` override | `lib/screen-renderers/table-utils.tsx:260` (`OverrideCell`) calls `expandVariableMacros` → `expandCellSelectionMacros` → `expandRowMacros` | A **separate** expansion path (`expandVariableMacros:159`, `expandRowMacros:182`, `expandCellSelectionMacros:209`) — not `substituteMacros`. |
| Transposed table override | `lib/screen-renderers/table-utils.tsx` (same `OverrideCell` reused) | Same path as table. |

### Chart adaptive formatting

The chart cell renders the same numeric values with adaptive unit scaling, but the logic lives as a private function inside `components/XYChart.tsx`:

- `formatValue(value, rawUnit, abbreviated, adaptiveTimeUnit?)` — `XYChart.tsx:57-93`. Dispatches on `isTimeUnit` / `isSizeUnit` / `isBitUnit` / `percent` / `degrees` / `boolean`, falling back to `value.toLocaleString()`.
- `formatStatValue(value, unit)` — `XYChart.tsx:96-101`. Same dispatch but uses `formatTimeValue` so each value picks its own best time unit (no shared chart-wide scale). **This is the variant template formatting needs.**

The underlying primitives are already module-scoped exports:

- `formatTimeValue`, `formatAdaptiveTime`, `getAdaptiveTimeUnit` — `lib/time-units.ts`
- `getAdaptiveSizeUnit`, `getAdaptiveBitUnit`, `normalizeUnit`, `isSizeUnit`, `isBitUnit` — `lib/units.ts`
- `isTimeUnit` — `lib/time-units.ts:39`

So the missing piece is a shared single-value formatter that both the chart and the template engine can call.

## Design

### 1. Shared formatter — `lib/format-value.ts` (new file)

Lift `formatValue` and `formatStatValue` out of `XYChart.tsx` into a new module. One export:

```ts
// lib/format-value.ts

/**
 * Format a numeric value with adaptive unit scaling, picking the best display
 * unit for this individual value. Used by both the chart cell (per-stat
 * formatting) and the template `format_value` function.
 */
export function formatValueWithUnit(value: number, rawUnit: string): string
```

`formatValueWithUnit` is the body of `formatStatValue` today: use `formatTimeValue` for time units, fall through to the size/bit/percent/degrees/boolean ladder. The size/bit/etc. ladder (today's `formatValue` body, minus the unused `adaptiveTimeUnit` parameter) lives as a private helper inside the module — `adaptiveTimeUnit` has no live caller in `XYChart.tsx` today (the chart's axis code at lines 625-635 and 761-769 formats inline; `formatValue`'s only caller is `formatStatValue`, which omits it), so the parameter is dropped during the lift. The chart imports `formatValueWithUnit` (replacing the existing `formatStatValue` invocations across `XYChart.tsx`); the template engine imports the same.

### 2. Template evaluator — extend `notebook-utils.ts`

Add a new entry point `evaluateTemplate` that walks the template **once**, left-to-right, dispatching at each position to function-call parsing, macro resolution, or literal char copy. Returns `{ text, warnings }` so callers can render unresolved-arg warnings.

```ts
export interface EvaluateTemplateCtx {
  variables: Record<string, VariableValue>
  /** Optional — when omitted, `$from`/`$to` macros are treated as unresolved
   *  (left in place as source). Some surfaces like `OverrideCell` accept an
   *  optional `timeRange` prop; in that case the caller passes `undefined`. */
  timeRange?: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
  /** Optional row dict (Table override path). When present, `$row.col` and
   *  `$row["col"]` resolve to the raw Arrow value. */
  row?: Record<string, unknown>
  /** Optional column-type map for RFC3339 stringification of timestamp values
   *  emitted as naked `$row.col` macros outside function-call args. Consumed
   *  only by the OverrideCell render path; `useTableWarnings` omits it because
   *  it discards rendered text and only collects warnings. */
  columnTypes?: Map<string, DataType>
}

export function evaluateTemplate(
  text: string,
  ctx: EvaluateTemplateCtx,
): { text: string; warnings: string[] }
```

Walker shape (pseudocode):

```
out = []
warnings = []
pos = 0
while pos < text.length:
  if identifier at pos and TEMPLATE_FUNCTIONS has that identifier
     and the char after the identifier is '(':
    parse `name(args)` span
    if the parse aborts (missing ')', bad arg, etc.):
      copy text[pos] and advance; continue       // not a real call
    resolve each arg:
      macro arg -> resolveMacroRaw(macro, ctx)   // raw value or undefined
      string literal -> strip surrounding quotes
      number literal -> Number(literal)
    if any arg is undefined OR the registry function returns undefined:
      emit one warning per problem
      copy the original call span verbatim       // no half-substituted state
      advance past ')'
    else:
      copy the formatted return value
      advance past ')'
  elif text[pos] == '$':
    try to parse a macro shape (the shapes substituteMacrosImpl handles —
    $from, $to, $cell[N].col, $cell.selected.col, $var.col, $var — PLUS
    $row.col and $row["col"] when ctx.row is set)
    note: $from/$to only resolve when ctx.timeRange is set; otherwise
    they are treated as unresolved and left in place as source
    if a shape matches and resolves:
      copy formatArrowValue(value, dataType)     // RFC3339 for timestamps;
                                                 // dataType source per shape:
                                                 //   $cell[N].col, $cell.selected.col → cellResults[cell].schema.fields.find(f=>f.name===col)?.type
                                                 //   $row.col, $row["col"]            → ctx.columnTypes?.get(col)
                                                 //   $variable, $variable.col, $from, $to → undefined (value is already a string)
      advance past macro
    elif a shape matches but is unresolved:
      copy the original macro source             // leave as-is
      advance past macro
    else:
      copy '$' and advance                       // bare '$' in user text
  else:
    copy text[pos] and advance
return { text: out.join(''), warnings }
```

The walker is the single template engine. No second pass, no sentinels, no restoration step: unresolved calls and unresolved macros stay as their original source text because the walker simply chooses to copy that span instead of substituting it. The function-call branch never mutates text *before* deciding success/failure (it only resolves args to JS values), so on failure there is nothing to undo.

`substituteMacros` (the SQL path) is **not** modified — function calls in v1 are template-only. If a future use case wants `format_value` inside SQL, it can build an SQL-aware variant that applies `escapeSqlValue` at the emission site; that's out of scope here.

### 3. Function-call grammar (v1)

```
call    := IDENT '(' arg ( ',' arg )* ')'
arg     := macro | string | number
macro   := '$' ident ( '[' ( digits | string ) ']' )? ( '.' ident ( '.' ident )? )?
string  := '\'' chars-without-quote '\''   |   '"' chars-without-quote '"'
number  := '-'? digits ( '.' digits )?
```

Constraints:
- No nested calls in v1 (`format_value(round($x), 'bytes')` is invalid).
- No arithmetic, no conditionals.
- String literals support both quote styles; the *opposite* quote may appear inside without escaping. (No backslash escapes for v1 — the only values that need quoting are short unit identifiers.)
- Numeric literals are passed through as JS `number` (no precision concerns at the source-text scale).
- The `[ string ]` macro form covers `$row["col-with-hyphens"]` for Table column overrides (Phase 4) — same syntax `expandRowMacros` accepts today.
- Whitespace allowed between tokens inside the argument list (e.g., `format_value( $x , 'bytes' )`). **No whitespace allowed between the function identifier and the opening `(`** — `format_value (x, 'bytes')` is *not* a call; the walker treats `format_value` as literal text and `(x, 'bytes')` as a parenthesized literal containing a naked `$`-less identifier. This matches the pseudocode in §2 ("the char after the identifier is `(`").

A function call is intercepted **only if** the identifier is in the registry. Unknown identifiers like `random_word($x)` cause the function-call branch to skip; the walker then copies `random_word` as ordinary text, hits `(`, copies it, hits `$x`, resolves it as a naked macro, etc. The net effect is `random_word(SUBSTITUTED_X)` — the same behavior the existing macro engine produces today, preserving backward compatibility for any template that happens to contain `identifier(...)` literally.

### 4. Argument resolution — raw value path

Add a `resolveMacroRaw(macroSpan, ctx): unknown` helper that mirrors the existing macro lookup logic in `substituteMacrosImpl` **but returns the underlying JS value** instead of a formatted string:

| Macro shape | Returns |
|---|---|
| `$from`, `$to` | `string` (ISO range value) when `ctx.timeRange` is set; `undefined` otherwise (matches the current `OverrideCell` behavior of skipping from/to merge when its `timeRange` prop is absent) |
| `$cell[N].column` | the Arrow value (`bigint` for timestamps and i64s, `number` for floats, etc.) |
| `$cell.selected.column` | same as above, from `cellSelections[cell][column]` |
| `$variable.column` | `string` (combobox column value) when `variables[var]` is a multi-column object; **`undefined`** when it's a simple string — matches `substituteMacrosImpl:392-394`, which returns `match` (leaves unresolved) for the simple-string case |
| `$variable` | `string` (or `getVariableString(value)` for multi-column) |
| `$row.col` / `$row["col"]` | the row's raw Arrow value — **only when `ctx.row` is set** (Table override path). On surfaces without a row, these shapes are not recognized and the walker treats them as unresolved. **A null or `undefined` cell value (or a missing column) is treated as unresolved** (returns `undefined`), matching the design's uniform "leave unresolved as source" rule and the parallel handling of null `$cell[N].col` in `substituteMacrosImpl:368`. |
| unresolved | `undefined` |

Important: `resolveMacroRaw` returns `undefined` for unresolved-selection (`$cell.selected.col` with no selection), unlike `substituteMacrosImpl` which returns `''` there. The walker keys on `undefined` to decide "leave the source span as-is and emit a warning." Because `resolveMacroRaw` is a fresh helper used only by the walker, this divergence costs nothing — `substituteMacrosImpl`'s SQL/overlay paths are unaffected.

For `format_value(value, unit)`:
- `value` arg is coerced — `bigint` → `Number(arg)` (loses precision >2^53 but acceptable for v1 byte/time ranges; size adaptive formatter already operates on `number`). Numeric strings → `Number(arg)`. Non-numeric → registry function returns `undefined`, the walker copies original source + warning.
- `unit` arg is coerced to string via `String(arg)`.

On an unresolved arg or `undefined` return from the registry function, the walker:
1. Copies the **original function-call source text** verbatim to the output (the walker hasn't mutated anything inside the span yet, so there's no half-substituted state to undo).
2. Emits a **warning** describing the problem (e.g., `format_value: $cell.selected.bytes is unresolved`, `format_value: expected 2 arguments, got 3`). Warnings accumulate in a deduplicated list returned alongside the rendered text.

### 5. Function registry

```ts
// lib/template-functions.ts (new file)
type TemplateFunction = (args: unknown[]) => string | undefined

export const TEMPLATE_FUNCTIONS: Record<string, TemplateFunction> = {
  format_value: (args) => {
    if (args.length !== 2) return undefined
    const [rawValue, rawUnit] = args
    const value = Number(rawValue)  // Number() handles bigint, number, and numeric strings
    if (!Number.isFinite(value)) return undefined
    return formatValueWithUnit(value, String(rawUnit ?? ''))
  },
}
```

A registry makes adding `format_duration`, `round`, `concat`, `clamp` later a one-line change with no walker edits — directly addresses the issue's extensibility argument.

### 6. Wiring

| Site | Change |
|---|---|
| `components/map/EventDetailPanel.tsx:59` | `substituteMacros(...)` → `evaluateTemplate(template, { variables: mergedVars, timeRange, cellResults, cellSelections })`. Destructure `{ text, warnings }`; render `text` as Markdown and surface `warnings` via `<TemplateWarningBanner>` above the body. |
| `lib/screen-renderers/cells/MarkdownCell.tsx:21` | Same swap. Markdown cells share the Map detail-panel rationale: the SQL-escape pass is wrong for plain Markdown content (it doubles single quotes), and authors should be able to call `format_value` in cell bodies. |
| `lib/screen-renderers/table-utils.tsx:OverrideCell` | Replace the legacy `expandVariableMacros` → `expandCellSelectionMacros` → `expandRowMacros` chain with a single `evaluateTemplate(format, { variables, timeRange, cellResults, cellSelections, row, columnTypes })` call. The walker resolves `$row.col`, `$cell.selected.col`, `$variable`, `$cell[N].col`, and `$from/$to` all in one pass — and it natively handles `format_value(...)` calls. The existing `allVariables` `useMemo` (which merged `from`/`to` into `variables`) is dropped because the walker resolves those from `ctx.timeRange` directly. The legacy `expand*Macros` exports and their tests are deleted (`OverrideCell` was their sole production caller). |
| Table warnings aggregation | Add a `useTableWarnings(data, overrides, ctx)` hook used by the table parents (TableCell, TableRenderer). The hook memoizes a dry-run pass that calls `evaluateTemplate` once per (row, override) tuple and aggregates `warnings` into a deduplicated `string[]`. **The hook's `ctx` deliberately omits `columnTypes`** — `columnTypes` only affects rendered text (RFC3339 stringification of naked `$row.col` timestamps), and the dry-run throws text away. Skipping it keeps the hook callable at the top of `TableCell` without depending on `table.schema`, which is only available past the early returns. Parents render `<TemplateWarningBanner warnings={warnings} />` above the table; no callback prop or `useEffect` glue on `TableBody`. Per-row banners would be noisy; one aggregate banner per table is enough. |

**Side effects of the Map/Markdown swaps:**

1. *Quote escaping:* the previous `substituteMacros` path doubled single quotes (SQL escape) on Markdown output — a latent bug, since neither surface is SQL. After the swap, quotes render verbatim. Default templates have no quotes, so this only affects user-customized templates.
2. *Naked unresolved `$cell.selected.col`:* under `substituteMacrosImpl` an unresolved selection macro collapsed to `''` (line 378/380), so `"a $cell.selected.col b"` rendered as `"a  b"`. The walker leaves the original macro source in place instead (and emits a warning), so the same template now renders as `"a $cell.selected.col b"`. This is intentional — the walker treats unresolved macros uniformly — and matches the function-call-arg behavior described in §4. **The same change applies to the table override path**: `expandCellSelectionMacros` (`table-utils.tsx:209-232`) also returns `''` for unresolved selection today, so column override `format` strings that bare-name `$cell.selected.col` will switch from collapsing to source-preserving + warning when `OverrideCell` swaps to `evaluateTemplate`.
3. *Variable boundary regex in table overrides:* the legacy `expandVariableMacros` (`table-utils.tsx:159-172`) uses `\\$${name}\\b` with no `(?![.[])` negative lookahead. Because `.` and `[` are non-word, `$metric.unit` (with `metric` a simple-string variable) currently expands to `<metric-value>.unit`, and `$metric[0]` currently expands to `<metric-value>[0]`. After the swap, the walker recognizes `$metric.unit` as the dotted-variable shape; per existing `substituteMacrosImpl` semantics a string-typed `metric` leaves the macro unresolved, so the same template now renders as `$metric.unit` (and emits a warning). The `$metric[0]` case fails to match any walker shape (no `[N].col` suffix) and is left as source. Net effect: any override that chains a literal `.suffix` or `[N]`-style suffix onto a simple-string variable switches from partial substitution to source-preserving + warning.
4. *Null `$row.col` (and `$row["col"]`) in table overrides:* the legacy `expandRowMacros` calls `formatValueForUrl` (`table-utils.tsx:50-65`), which returns `''` when `row[col]` is `null`/`undefined` or the column is missing. After the swap, per §4 above the walker treats those cases as unresolved and copies the original `$row.col` source verbatim (and emits one warning per row+column tuple, deduplicated by `useTableWarnings`). Net behavior: null cells in a custom-formatted column switch from rendering as empty cells to rendering as the literal macro text with a banner explaining why — same source-preserving rule the rest of the walker uses. Affects override templates that ever encounter a null/missing column value.

The testing strategy below pins these behavior changes.

### 6a. Warning banner contract

`evaluateTemplate` returns `{ text, warnings }`:
- `text` is the rendered output; unresolved calls and unresolved macros appear as their original source.
- `warnings` is `string[]` — one entry per problem in source order, deduplicated within a single `evaluateTemplate` call.

Banner UI is a thin reusable component (e.g., `<TemplateWarningBanner warnings={...} />`) used by both `EventDetailPanel`/`MarkdownCell` (single-template surfaces) and the table parents (which aggregate via `useTableWarnings`). Empty `warnings` renders nothing.

## Implementation Steps

### Phase 1 — Shared formatter

1. Create `analytics-web-app/src/lib/format-value.ts` with `formatValueWithUnit` (single export, per *Design §1*). Body lifted from `XYChart.tsx:57-101`; the size/bit/etc. ladder lives as a private helper in the same file.
2. Update `components/XYChart.tsx` to import the new helpers and delete the local `formatValue` / `formatStatValue`. No behavior change.
3. Add unit tests at `lib/__tests__/format-value.test.ts` covering: time units (ns / µs / ms / s / min / h / d), size units (bytes / KB / MB / GB / TB), bit units, `percent`, `degrees`, `boolean`, and the unitless fallback.

### Phase 2 — Template evaluator

4. Create `lib/template-functions.ts` with the `TEMPLATE_FUNCTIONS` registry and `format_value` implementation.
5. In `lib/screen-renderers/notebook-utils.ts`:
   - Add `resolveMacroRaw(macroSpan, ctx): unknown` per *Design §4*. Reuses the same regexes as `substituteMacrosImpl` for parsing a macro shape but returns the raw value (or `undefined` when unresolved — including the `$cell.selected.col`-with-no-selection case, which is *not* mapped to `''` here).
   - Add `evaluateTemplate(text, ctx): { text: string; warnings: string[] }` as the single-pass walker described in *Design §2*. The walker has three branches per character position: (a) identifier-in-registry followed by `(` → function call; (b) `$` → macro; (c) literal copy. Per-call outcomes:
     - **Parse abort** (e.g., missing `)`, malformed arg): walker falls back to literal copy at the current position; not treated as a call.
     - **Unknown function name**: function-call branch doesn't fire (registry miss). Walker copies the identifier as text; any `$macro` inside the following parens is resolved normally when the walker reaches it.
     - **Unresolved macro arg** (any arg resolves to `undefined`): copy the original call span verbatim; emit one warning per unresolved arg naming the macro shape.
     - **Registry function returns `undefined`** (wrong arity, non-finite numeric coercion, etc.): copy the original call span verbatim; emit one warning naming the function and the reason (e.g., `"format_value: argument 1 is not a numeric value"`, `"format_value: expected 2 arguments, got N"`). Keeps user-visible output identical to the source instead of leaking the literal string `"undefined"`.
     - **Success**: copy the formatted return value.
   - Warnings are deduplicated within a single `evaluateTemplate` call (a `Set<string>` collected during the walk, returned as `string[]` in insertion order).
6. Add tests at `lib/screen-renderers/__tests__/notebook-utils.test.ts`:
   - `format_value(3678630912, 'bytes')` → `"3.4 GB"` (matches existing chart formatter: size units other than bytes render with 1 decimal).
   - `format_value($metric_avg, $metric.unit)` with a multi-column variable.
   - `format_value($cell.selected.bytes, 'bytes')` with a selected row.
   - `format_value($cell[0].duration_ns, 'nanoseconds')` — BigInt arg path.
   - Unknown function name `foo(1,2)` — `$macros` inside resolve normally; literal text outside stays.
   - Unresolved macro arg `format_value($missing, 'bytes')` — original call source emitted **and** a warning naming `$missing`.
   - Unresolved selection arg `format_value($cell.selected.bytes, 'bytes')` with no selection — original call source emitted (the walker never substitutes inside the call, so there is no `format_value(, 'bytes')` corruption) **and** a warning emitted.
   - String literals containing commas: `format_value($x, 'GB, please')`.
   - Mixed: `format_value($x, 'bytes') extra $y` — call replaced, naked `$y` substituted.
   - Multiple unresolved calls: warnings deduplicated when the same unresolved arg appears in more than one call.
   - Naked-macro behavior: `evaluateTemplate("a $variable b", { variables: { variable: 'X' }, … })` → `"a X b"` (parity with `substituteMacrosRaw`).
   - Naked unresolved selection (regression-pin): `evaluateTemplate("a $cell.selected.col b", { cellSelections: {}, … })` → `"a $cell.selected.col b"` (macro left as-is, **not** collapsed to `"a  b"` like `substituteMacrosImpl` would). Pins the §6 side-effect #2 behavior change.
   - Quote-escape regression: `evaluateTemplate("msg: $search", { variables: { search: "it's working" }, … })` produces `"msg: it's working"` (single quotes preserved, **not** doubled). Pins the §6 side-effect #1 behavior change.

### Phase 3 — Wire into Map and Markdown cells

7. `components/map/EventDetailPanel.tsx:59`: swap `substituteMacros` → `evaluateTemplate`. Destructure `{ text, warnings }`; render `text` as Markdown and render `warnings` (if any) in a banner above the body.
8. Create `components/TemplateWarningBanner.tsx` — a small component that renders an amber-bordered list of warning strings, hidden when the list is empty. Used by the Map panel, the Markdown cell, and the table parents.
9. `lib/screen-renderers/cells/MarkdownCell.tsx:21`: swap `substituteMacros` → `evaluateTemplate`. Same shape — destructure `{ text, warnings }`, render the banner above the existing Markdown body.
10. Manual test: notebook with a Map cell whose query returns a `bytes` column; template uses `format_value($bytes, 'bytes')`; verify adaptive output in the detail panel. Add a Markdown cell with body `Size: format_value($total_bytes, 'bytes')` referencing a variable; verify the cell renders the adaptive output.

### Phase 4 — Wire into Table format overrides

11. Refactor `OverrideCell` (`table-utils.tsx:260`) to call `evaluateTemplate(format, { variables, timeRange, cellResults, cellSelections, row, columnTypes })` and render `text` as Markdown. Pass `variables` directly — the existing `allVariables = {...variables, from: timeRange.begin, to: timeRange.end}` `useMemo` is no longer needed because the walker resolves `$from`/`$to` natively from `ctx.timeRange`; remove that `useMemo`. Delete the inline `expandVariableMacros → expandCellSelectionMacros → expandRowMacros` chain. The walker resolves all the same shapes and additionally supports `$cell[N].col` inside overrides.
12. Delete the legacy `expandVariableMacros`, `expandCellSelectionMacros`, `expandRowMacros` exports from `table-utils.tsx` and the corresponding tests in `__tests__/table-utils.test.tsx`. `OverrideCell` was their sole production caller; the walker tests cover the same macro shapes.
13. Add `useTableWarnings(data, overrides, ctx): string[]` to `table-utils.tsx`, plus module-level stable-empty constants `const EMPTY_VARIABLES: Record<string, VariableValue> = {}`, `const EMPTY_CELL_RESULTS: Record<string, Table> = {}`, `const EMPTY_CELL_SELECTIONS: Record<string, Record<string, unknown>> = {}`, `const EMPTY_OVERRIDES: ColumnOverride[] = []` (all exported so callers without those fields can pass the same identity each render). The hook is memoized on `[data, overrides, ctx.variables, ctx.timeRange, ctx.cellResults, ctx.cellSelections]`; passing fresh `{}` or `[]` literals each render would invalidate the memo every render and defeat the cap's amortization, so callers that lack a real value must pass these constants. **Callers must also pass a stable `data` reference** — see the `TableRenderer` wiring note about wrapping `streamQuery.getTable()` in a `useMemo`, since that function constructs a fresh `Table` on every invocation. The hook iterates each (row, override) tuple up to a cap (`TABLE_WARNINGS_ROW_CAP`, defaulting to 500), calls `evaluateTemplate` for warnings only (return value is discarded — same `evaluateTemplate` the cell uses, just thrown away after extracting warnings), and aggregates into a deduplicated `string[]`. The hook's `ctx` **does not include `columnTypes`** — `columnTypes` only affects rendered text (RFC3339 stringification of naked `$row.col` timestamps), and the dry-run discards text. Skipping it means the hook doesn't need `table.schema`, so it can be called at the top of `TableCell` without depending on values only available past the early returns. The hook **accepts `data` that may be `null`/`undefined` or have `numRows === 0`** and returns an empty array in those cases — this lets parents call it unconditionally at the top of the component, before any early returns, so it stays compliant with React's rules of hooks. If `data.numRows > TABLE_WARNINGS_ROW_CAP`, append a final synthetic entry `"More rows were not scanned for warnings (capped at <N>)."` so users know coverage is partial. **Why the cap matters:** `TableRenderer.tsx:404` passes the full unsliced query result (no pagination at that layer, unlike `cells/TableCell.tsx:167` which renders `TableBody` with `slicedData` from `usePagination` at `cells/TableCell.tsx:91`), so without the cap a 10k-row screen table with 5 overrides would trigger 50k walker invocations on each memo invalidation. No callbacks, no `useEffect` — the hook returns the array synchronously and the parent renders the banner directly.

    **Per-caller wiring** (three `TableBody` call sites — only the parents change; `TableBody` itself does not gain new props):
    - `cells/TableCell.tsx` (notebook table cell — full notebook context): call `useTableWarnings(table, overrides, ctx)` near the top of the component, **before** the existing early returns at lines 104-117 (`status === 'loading'` and `!table || numRows === 0`). Pass the unsliced `table` (which may be `undefined`) rather than `slicedData` — `slicedData` is only computed after the early returns and is unsuitable as a hook argument. Scanning all rows up to the cap also gives users warnings for rows not on the current page. The cell's `table = data[0]` comes from React state managed by the notebook execution layer, so its reference is stable across renders — no `useMemo` needed here (unlike `TableRenderer`). The `overrides` value is computed at line 34 as `(options?.overrides as ColumnOverride[] | undefined) || []` — change the `|| []` to `|| EMPTY_OVERRIDES` so the empty case doesn't invalidate the memo each render. Render `<TemplateWarningBanner warnings={warnings} />` above the `<table>` in the success-path JSX. This is the primary surface where authors write `format_value($row.col, …)`. `OverrideCell`'s own `columnTypes` useMemo is unaffected — it stays where it is today (table-utils.tsx:262-268), computed during the per-cell render after the row is known to exist.
    - `TableRenderer.tsx` (top-level screen table — no `variables` in scope): the existing `TableBody` lives inside `renderContent()` (defined at `TableRenderer.tsx:363`) past early returns at lines 366–372, so `useTableWarnings` cannot be called at the `TableBody` site without violating React's rules of hooks. Instead, **wrap the `streamQuery.getTable()` call in a `useMemo`** at the top of the component body — `useStreamQuery.ts:115-118` defines `getTable` as `() => new Table(batchesRef.current)`, which returns a fresh `Table` reference on every call, so without memoization the hook's `[data, ...]` deps invalidate every render and the cap becomes the per-render cost rather than amortized. Then call `useTableWarnings(memoizedTable, tableConfig.overrides ?? EMPTY_OVERRIDES, ctx)`, threading `memoizedTable` and `warnings` into `renderContent` via closure (and replacing the redundant second `streamQuery.getTable()` call at line 364). Use the module-level `EMPTY_VARIABLES` / `EMPTY_CELL_RESULTS` / `EMPTY_CELL_SELECTIONS` / `EMPTY_OVERRIDES` constants (see step 13 below) for `ctx` so the hook's `useMemo` deps stay stable. Banner placement: render `<TemplateWarningBanner warnings={warnings} />` inside the `<div className="flex-1 overflow-auto ...">` at `TableRenderer.tsx:383`, immediately before the `<table>`. Even without notebook variables, screen-level tables can use `format_value($row.col, 'bytes')`, so warnings still need surfacing when override args fail to resolve.

      ```ts
      // TableRenderer.tsx — replace the bare `streamQuery.getTable()` call at line 195
      const table = useMemo(
        () => streamQuery.getTable(),
        [streamQuery.batchCount, streamQuery.isComplete],
      )
      const warnings = useTableWarnings(
        table,
        tableConfig.overrides ?? EMPTY_OVERRIDES,
        {
          variables: EMPTY_VARIABLES,
          cellResults: EMPTY_CELL_RESULTS,
          cellSelections: EMPTY_CELL_SELECTIONS,
          timeRange,
        },
      )
      ```
    - `cells/ReferenceTableCell.tsx:132` (reference table — empty `cellSelections`/`cellResults`, no overrides): skip the hook entirely. Reference tables don't accept column overrides today, so no `format_value` calls run. If overrides are ever added, switch to match `TableCell`.

    **Cost note:** the dry-run pass does duplicate the per-cell `evaluateTemplate` work that `OverrideCell` will do during render. The `TABLE_WARNINGS_ROW_CAP` (default 500) bounds the worst case across all callers (since the hook always scans the unsliced data). If profiling ever shows the duplication as a bottleneck, the hook and `OverrideCell` can share a memoized `Map<(format, rowIdx), Result>` cache.
14. Add tests at `lib/screen-renderers/__tests__/table-utils.test.tsx` covering: (a) a column override that calls `format_value($row.bytes, 'bytes')` rendering adaptively, (b) a column override with an unresolved arg (e.g., `format_value($cell.selected.missing, 'bytes')` with no selection) leaving the original source text and emitting a warning, (c) `useTableWarnings` deduplicating warnings across rows, (d) a naked unresolved `$cell.selected.col` in a column override `format` string — pins side-effect §6 #2 for the table path (`expandCellSelectionMacros` would previously collapse to `''`; walker now preserves source + warns), and (e) a column override with `$row.col` for a row whose `col` is `null` (or whose column is missing) — pins side-effect §6 #4: walker preserves the `$row.col` source text and emits a warning, where `expandRowMacros` previously rendered `''`.

### Phase 5 — Documentation

15. Update `mkdocs/docs/web-app/notebooks/variables.md`:
    - Add a *Template Functions* subsection under *SQL Macro Substitution* describing the v1 surface and the `format_value(value, unit)` signature with a few examples.
    - Note the unit vocabulary (point to `lib/units.ts` aliases).
    - Note SQL queries do **not** support function calls in v1.

### Phase 6 — Checks

16. From `analytics-web-app/`: `yarn lint`, `yarn type-check`, `yarn test`.

## Files to Modify

| File | Change |
|---|---|
| `analytics-web-app/src/lib/format-value.ts` *(new)* | Shared `formatValueWithUnit` |
| `analytics-web-app/src/lib/__tests__/format-value.test.ts` *(new)* | Unit tests for the shared formatter |
| `analytics-web-app/src/lib/template-functions.ts` *(new)* | `TEMPLATE_FUNCTIONS` registry + `format_value` impl |
| `analytics-web-app/src/components/XYChart.tsx` | Delete local `formatValue` / `formatStatValue`; import from shared module |
| `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` | Add `resolveMacroRaw` and `evaluateTemplate` (single-pass walker) |
| `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts` | Tests for `evaluateTemplate` and `format_value` |
| `analytics-web-app/src/components/map/EventDetailPanel.tsx` | Swap `substituteMacros` → `evaluateTemplate`; render warning banner |
| `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx` | Swap `substituteMacros` → `evaluateTemplate`; render warning banner |
| `analytics-web-app/src/components/TemplateWarningBanner.tsx` *(new)* | Reusable warning-banner component used by Map panel, Markdown cell, and table parents |
| `analytics-web-app/src/lib/screen-renderers/table-utils.tsx` | Replace `OverrideCell`'s legacy `expand*Macros` chain with `evaluateTemplate`; delete `expandVariableMacros` / `expandCellSelectionMacros` / `expandRowMacros`; add `useTableWarnings` hook |
| `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` | Call `useTableWarnings`; render `<TemplateWarningBanner>` above the table |
| `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx` | Call `useTableWarnings`; render `<TemplateWarningBanner>` above the table |
| `analytics-web-app/src/lib/screen-renderers/__tests__/table-utils.test.tsx` | Delete legacy `expand*Macros` tests; add `format_value($row.col, 'unit')` test, unresolved-arg test, `useTableWarnings` dedup test |
| `mkdocs/docs/web-app/notebooks/variables.md` | Document template functions |

## Trade-offs

### Single-pass walker vs. multi-pass + sentinels

**Chosen: single-pass walker.** A walker resolves function calls and macros in one left-to-right scan, so unresolved spans simply stay as their original source — no two-pass coordination, no opaque sentinel tokens, no restoration step. The earlier draft of this plan used a two-pass `evaluateFunctionCalls` → `substituteMacrosRaw` design with sentinels to hide unresolved calls from the second pass; that complexity existed only because `substituteMacrosImpl` returns `''` for an unresolved `$cell.selected.col` (which would have corrupted unresolved calls in pass 2). The walker sidesteps both problems by never running a second pass over text it already touched.

### Sibling `evaluateTemplate` vs. extending `substituteMacros`

**Chosen: sibling function.** The issue explicitly leaves both open. A sibling keeps the SQL path (`substituteMacros`) frozen — zero risk to existing query templates — and avoids paying the function-call parsing cost for every SQL substitution (a far hotter path than Markdown rendering).

### Function-call grammar vs. suffix pipe

The issue rules this out at length (composability, type preservation, extensibility). This plan inherits that conclusion.

### Registry vs. switch statement

**Chosen: registry.** Adding `format_duration`, `round`, `concat`, `clamp` later should be one entry in a table, not a walker edit. Cost is negligible — one indirection.

### Raw BigInt → number coercion in `format_value`

**Chosen: coerce to `number` in the function impl.** `getAdaptiveSizeUnit` operates on `number`. Values up to `2^53` (≈9 PB in bytes) fit exactly; larger values lose low-order bits but remain accurate to ~15 significant digits, which is fine for adaptive scaling that displays 3 digits. Future work could add a BigInt-aware byte formatter if multi-petabyte precision is ever needed.

### Row column precision (Map)

`$cell[N].col` and `$cell.selected.col` are lossless on every surface: `resolveMacroRaw` reads from `cellResults[name].get(idx)[col]` and `cellSelections[name][col]` directly, returning the Arrow primitive (BigInt for i64, number for f64).

`$row.col` is lossless for **Table** because `OverrideCell` already receives the raw `row: Record<string, unknown>` and the walker reads from it through `ctx.row`.

The one path that **isn't** lossless is `$x` (row column merged into vars) in **Map** templates: `materializeRow` in `components/map/overlay.ts:565` produces `Row = Record<string, string>` (see `overlay.ts:16`) — every cell goes through `formatArrowValue`, so by the time `EventDetailPanel` merges row into variables, values are strings. For `format_value` on such an arg, the walker reads a string and `Number()`-coerces it. Round-trip is exact for `number` and exact for `bigint` decimal stringification within the safe-integer range; it loses low-order bits for i64 values above 2^53.

That's adequate for v1 because adaptive scaling displays ~3 significant digits — a 9 PB+ byte count formatted as "3.4 GB" doesn't notice the lost bits. If lossless byte handling for >2^53 ever matters here, a follow-up can thread the raw Arrow row alongside the stringified `Row` to `EventDetailPanel`. Not worth doing speculatively.

### Quote-escape rules in v1

**No backslash escapes.** A unit identifier is at most 10 chars (`'gigabytes'`); supporting `'\\n'` invites parser complexity for no real use case. If a template ever needs a literal quote, switch the outer quote. Document this in the docs page.

### Deleting the legacy `expand*Macros` chain

**Chosen: delete.** `OverrideCell` is the sole production caller; once it switches to `evaluateTemplate`, the three functions and their tests are dead code. The walker covers every macro shape they handled and a few more (`$cell[N].col`, `$from`/`$to`), so removal also brings the table override path to feature parity with the rest of the template engine.

### Acknowledgement — most of the complexity comes from error reporting

Stripped of the warnings/banner machinery, the work this plan describes is small: a shared `formatValueWithUnit`, a single-pass walker that resolves macros to raw values, and a one-entry function registry. The bulk of the design surface — the `{ text, warnings }` return shape, dedup rules, `useTableWarnings` hook with row caps, per-caller wiring, the `<TemplateWarningBanner>` component, multiple "preserve original source on failure" code paths — exists because the *existing* macro engine handles failure poorly (unresolved `$cell.selected.col` silently collapses to `''`, no diagnostics for unresolved variables, no surface to communicate problems to the author). v1's scope grew to fix those gaps alongside introducing function calls, since once a `format_value(...)` call can plausibly fail at render time, leaving authors with silent empty output was no longer acceptable. A future cleanup could pull the error-management primitives (warning bag, banner, hook) into a shared layer reused by the SQL macro path too, so this is not strictly net-new infrastructure.

## Documentation

`mkdocs/docs/web-app/notebooks/variables.md` — add a *Template Functions* subsection under *SQL Macro Substitution*:

- v1 functions: `format_value(value, unit)`.
- Where it works: Markdown templates (Map detail panel, notebook Markdown cells, table column overrides). **Not** SQL queries.
- Unit vocabulary: same aliases the chart understands (point readers to seconds/ms/bytes/percent/etc.).
- Mention that args may be macros (resolved before the function runs) or string literals in single or double quotes.

No changes needed to `cell-types.md` or `execution.md`.

## Testing Strategy

### Unit tests

- `lib/__tests__/format-value.test.ts`: every unit branch (time / size / bit / percent / degrees / boolean / unitless fallback). Identical input/output to today's chart behavior — these tests double as regression coverage for the chart refactor.
- `lib/screen-renderers/__tests__/notebook-utils.test.ts`:
  - Successful function calls with each macro shape (`$variable`, `$variable.column`, `$cell[N].column`, `$cell.selected.column`).
  - String literal args, mixed macro+literal, both quote styles.
  - Unknown function names — `$macros` inside still resolve.
  - Unresolved macro args — original call source preserved, warning emitted.
  - Unresolved selection (`$cell.selected.col` with no selection inside `format_value(...)`) — original call source preserved (the walker never produces the `format_value(, 'bytes')` corruption that the two-pass design would have).
  - BigInt arg (timestamp column → seconds unit).
  - Multiple function calls in one template.
  - Function call followed by normal `$variable` substitution outside it.
  - Naked macro behavior parity with `substituteMacrosRaw` for resolved cases.
  - Quote-escape regression: `evaluateTemplate("msg: $search", { variables: { search: "it's working" }, … })` produces `"msg: it's working"` (single quotes preserved, **not** doubled). Pins the Map/Markdown swap behavior change so the SQL escape never silently leaks back in.
- `lib/screen-renderers/__tests__/table-utils.test.tsx`: column override with `format_value($row.col, 'bytes')` renders adaptive text; unresolved-arg override preserves source + emits a warning; `useTableWarnings` deduplicates warnings across rows.

### Manual tests

1. Notebook with Map cell. Query: `SELECT NOW() as time, 0 as x, 0 as y, 0 as z, 3678630912 as bytes_used`. Detail template: `**Memory:** format_value($bytes_used, 'bytes')`. Verify panel renders `3.4 GB` (size units other than `bytes` render with 1 decimal, matching the chart formatter).
2. Notebook with a variable cell `metric` whose query returns `(name, unit)` rows (e.g. `SELECT 'memory' AS name, 'bytes' AS unit UNION ALL SELECT 'latency', 'seconds'`) — a multi-column variable. Add a Map cell whose query exposes both a value column and the selected metric, e.g. `SELECT NOW() AS time, 0 AS x, 0 AS y, 0 AS z, 3678630912 AS metric_value`. Detail template: `**Value:** format_value($metric_value, $metric.unit)`. Switch the `metric` combobox between `memory` and `latency` and verify the rendered output adapts (`3.4 GB` vs. an adaptive-time format on the same numeric value).
3. Table cell with `bytes_used` column. Column override: `format_value($row.bytes_used, 'bytes')`. Verify each row formats adaptively.

## Out of Scope (v1, per issue)

- Arithmetic in templates (`$a + $b`)
- User-defined functions
- Conditional expressions
- Function calls inside SQL templates
- Nested function calls
- Function-call argument validation at *edit time*: `validateMacros` (`notebook-utils.ts:436`) and `validateFormatMacros` (`table-utils.tsx:141`) are intentionally **not** updated. Inner `$macro` arguments still validate because the validators scan `$…` patterns anywhere in the text. v1 doesn't check function arity, function-name existence, or unit-name validity at edit time. At *render time*, unresolved-arg calls render as their original source text and surface a warning via the banner described in *Wiring*; unknown function names still pass through unchanged with no warning (matching the "pass through unknown" rule). Adding arity/unit checks to the editor validators is a clean future increment.

## Open Questions

None — ship Phases 1-6 as one PR (per follow-up decision: bundled delivery, not split).
