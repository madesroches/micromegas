# Template Function Calls — `format_value` Plan

## Issue Reference
- [#1086](https://github.com/madesroches/micromegas/issues/1086) — Support function-call expressions in template macro engine

## Overview

Add a tiny function-call expression layer to the template macro engine so values interpolated into Markdown templates (Map `detailTemplate`, Table `format` overrides) can be rendered with the same adaptive unit formatting the chart cell already uses. v1 surface is a single function — `format_value(value, unit)` — that reuses the chart's adaptive formatters. The evaluator runs *before* normal macro substitution and resolves macro arguments to **raw JS/Arrow values** (not strings), so byte counts and large floats keep full precision.

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
| Table column `format` override | `lib/screen-renderers/table-utils.tsx:260` (`OverrideCell`) calls `expandVariableMacros` → `expandCellSelectionMacros` → `expandRowMacros` | A **separate** expansion path (line 159-202) — not `substituteMacros`. |
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

Lift `formatValue` and `formatStatValue` out of `XYChart.tsx` into a new module. Two exports:

```ts
// lib/format-value.ts

/**
 * Format a numeric value with adaptive unit scaling, picking the best display
 * unit for this individual value. Used by both the chart cell (per-stat
 * formatting) and the template `format_value` function.
 */
export function formatValueWithUnit(value: number, rawUnit: string): string

/**
 * Chart-wide variant: when an `AdaptiveTimeUnit` is provided (computed once
 * per chart axis from p99/max), all values share the same time unit.
 * Used only by the chart cell.
 */
export function formatValueWithScale(
  value: number,
  rawUnit: string,
  abbreviated?: boolean,
  adaptiveTimeUnit?: AdaptiveTimeUnit,
): string
```

`formatValueWithUnit` is the body of `formatStatValue` today (use `formatTimeValue` for time units; fall through to the size/bit/percent/degrees/boolean ladder). `formatValueWithScale` is the body of the existing `formatValue` (takes the precomputed adaptive unit) — kept as an export so `formatValueWithUnit` can delegate to it internally for non-time units, matching today's structure. The chart imports `formatValueWithUnit` (replacing its calls to `formatStatValue`); the template engine imports the same. `formatValueWithScale` has no external callers in v1 — exported only because the layered helper relationship between the two formatters is a stable interface worth keeping discoverable.

### 2. Function-call evaluator — extend `notebook-utils.ts`

Add a new entry point `evaluateTemplate` that runs *before* macro substitution and resolves function calls. Returns `{ text, warnings }` so callers can render unresolved-arg warnings (see *Argument resolution* and *Wiring* below). Composition:

```ts
export function evaluateTemplate(
  text: string,
  variables: Record<string, VariableValue>,
  timeRange: { begin: string; end: string },
  cellResults: Record<string, Table>,
  cellSelections: Record<string, Record<string, unknown>>,
): { text: string; warnings: string[] } {
  // Pass 1: function-call evaluation. Resolved calls are spliced in as
  // formatted strings; unresolved-arg calls are replaced with opaque
  // sentinel tokens so pass 2 can't partially substitute them.
  const { text: withSentinels, warnings, sentinels } =
    evaluateFunctionCalls(text, variables, timeRange, cellResults, cellSelections)
  // Pass 2: normal macro substitution (raw mode — Markdown output, not SQL).
  const substituted = substituteMacrosRaw(withSentinels, variables, timeRange, cellResults, cellSelections)
  // Restore sentinels back to their original `format_value(...)` source text.
  return { text: restoreSentinels(substituted, sentinels), warnings }
}

/** Restore sentinel tokens back to their original `format_value(...)` source
 *  text. Exported so the Table override path can reuse the same step after its
 *  own macro chain instead of inlining the loop. */
export function restoreSentinels(text: string, sentinels: Map<string, string>): string {
  let restored = text
  for (const [token, source] of sentinels) restored = restored.replaceAll(token, source)
  return restored
}
```

`substituteMacros` (the SQL path) is **not** modified — function calls in v1 are template-only. If a future use case wants `format_value` inside SQL, it can compose `evaluateTemplate` with a SQL-escape pass; that's out of scope here.

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
- Whitespace allowed between tokens.

A function call is intercepted **only if** the identifier is in the registry. Unknown identifiers like `random_word($x)` are passed through unchanged — they'll go through normal macro substitution in pass 2 (which only touches `$…` tokens, leaving the literal `random_word(...)` text intact). This preserves backward compatibility for any template that happens to contain `identifier(...)` literally.

### 4. Argument resolution — raw value path

Add a `resolveMacroRaw(macro, variables, timeRange, cellResults, cellSelections): unknown` helper that mirrors the existing macro lookup logic in `substituteMacrosImpl` **but returns the underlying JS value** instead of a formatted string:

| Macro shape | Returns |
|---|---|
| `$from`, `$to` | `string` (ISO range value) |
| `$cell[N].column` | the Arrow value (`bigint` for timestamps and i64s, `number` for floats, etc.) |
| `$cell.selected.column` | same as above, from `cellSelections[cell][column]` |
| `$variable.column` | `string` (combobox column values are strings today) |
| `$variable` | `string` (or `getVariableString(value)` for multi-column) |
| `$row.col` / `$row["col"]` | the row's raw Arrow value — **only when `resolveMacroRaw` is called with a `row` argument** (Table override path, Phase 4). On surfaces without a row argument, these shapes are not recognized. |
| unresolved | `undefined` |

For `format_value(value, unit)`:
- `value` arg is coerced — `bigint` → `Number(arg)` (loses precision >2^53 but acceptable for v1 byte/time ranges; size adaptive formatter already operates on `number`). Numeric strings → `Number(arg)`. Non-numeric → returns the original function-call text unchanged.
- `unit` arg is coerced to string via `String(arg)`.

If any arg is `undefined` (unresolved macro), the evaluator:
1. Replaces the function-call span with a **sentinel** that the second pass (`substituteMacrosRaw`) will not touch — protecting the call from partial substitution. After pass 2, the sentinel is rewritten back to the original `format_value(...)` source text so the user sees the unresolved expression verbatim in the rendered output.
2. Emits a **warning** describing the unresolved arg (e.g., `format_value: $cell.selected.bytes is unresolved`). Warnings accumulate in a list returned alongside the rendered text.

The sentinel step is needed because `substituteMacrosImpl` is inconsistent across macro shapes — `$cell.selected.col` resolves to `''` (not `match`) when no selection exists (`notebook-utils.ts:378,380`), which would otherwise leak through to pass 2 and produce half-substituted text like `format_value(, 'bytes')`. The sentinel guarantees the "left as-is" contract for every macro shape, not just the four that already return `match`.

`evaluateTemplate` returns `{ text: string; warnings: string[] }`. Callers display warnings as a non-blocking banner above the cell's rendered content (see *Wiring* below).

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

A registry makes adding `format_duration`, `round`, `concat`, `clamp` later a one-line change with no further evaluator edits — directly addresses the issue's extensibility argument.

### 6. Wiring

| Site | Change |
|---|---|
| `components/map/EventDetailPanel.tsx:59` | `substituteMacros(...)` → `evaluateTemplate(...)`. Render the returned `warnings` as a banner at the top of the panel (above the Markdown body). Banner styling: yellow/amber accent, one line per warning, dismissable only by fixing the template. |
| `lib/screen-renderers/table-utils.tsx:OverrideCell` | Run `evaluateFunctionCalls` *before* the existing `expandVariableMacros` → `expandCellSelectionMacros` → `expandRowMacros` chain, passing the raw row dict so `$row.col` resolves losslessly. See *Trade-offs* below for the resolution. Per-cell warnings would be noisy; aggregate distinct warnings at the table level and render a single banner above the table body (TableRenderer / TableCell). |

Both Map and Table wiring ship in the same PR (see *Open Questions*).

### 6a. Warning banner contract

`evaluateTemplate` returns `{ text, warnings }`:
- `text` is the rendered output (sentinel-protected unresolved calls restored to their original source form).
- `warnings` is `string[]` — one entry per unresolved function-call arg, in source order, deduplicated.

Banner UI is a thin reusable component (e.g., `<TemplateWarningBanner warnings={...} />`) used by both EventDetailPanel and the table renderer. Empty `warnings` renders nothing.

## Implementation Steps

### Phase 1 — Shared formatter

1. Create `analytics-web-app/src/lib/format-value.ts` with `formatValueWithUnit` and `formatValueWithScale`. Body lifted from `XYChart.tsx:57-101`.
2. Update `components/XYChart.tsx` to import the new helpers and delete the local `formatValue` / `formatStatValue`. No behavior change.
3. Add unit tests at `lib/__tests__/format-value.test.ts` covering: time units (ns / µs / ms / s / min / h / d), size units (bytes / KB / MB / GB / TB), bit units, `percent`, `degrees`, `boolean`, and the unitless fallback.

### Phase 2 — Function-call evaluator

4. Create `lib/template-functions.ts` with the `TEMPLATE_FUNCTIONS` registry and `format_value` implementation.
5. In `lib/screen-renderers/notebook-utils.ts`:
   - Add `resolveMacroRaw(macroText, variables, timeRange, cellResults, cellSelections): unknown`. Reuses the same regexes as `substituteMacrosImpl` for parsing the macro shape but returns the raw value (or `undefined` when unresolved — including the `$cell.selected.col`-with-no-selection case, which differs from `substituteMacrosImpl`'s `''` return).
   - Add a small parser `parseFunctionCalls(text)` that walks `text` and returns `{start, end, name, args: string[]}` records. Each arg is a substring (macro, quoted literal, or numeric literal) without surrounding whitespace. Handle both quote styles.
   - Add `evaluateFunctionCalls(text, variables, timeRange, cellResults, cellSelections): { text: string; warnings: string[]; sentinels: Map<string, string> }` that calls `parseFunctionCalls`, looks up each name in `TEMPLATE_FUNCTIONS`, resolves args (`resolveMacroRaw` for macros, strip quotes for string literals, `Number()` for numeric literals), and splices the result back. Unknown function names are passed through unchanged. **Unresolved-arg calls are replaced with an opaque sentinel token** (e.g., ` FNCALL${n} ` — a string `substituteMacrosRaw` can't match because it contains no `$`) and a warning is recorded for each unresolved arg. The original source text is stored in `sentinels` keyed by sentinel.
   - Add the public `evaluateTemplate` wrapper that returns `{ text, warnings }`:
     1. Call `evaluateFunctionCalls` → `{text, warnings, sentinels}`.
     2. Call `substituteMacrosRaw` on the sentinel-protected text.
     3. Restore each sentinel back to its original source text.
     4. Return `{ text: restored, warnings }`.
   - Export `restoreSentinels(text, sentinels): string` (the loop body from step 3) so the Table path in Phase 4 can reuse it after its own macro chain instead of inlining the loop.
6. Add tests at `lib/screen-renderers/__tests__/notebook-utils.test.ts`:
   - `format_value(3678630912, 'bytes')` → `"3.4 GB"` (matches existing chart formatter: size units other than bytes render with 1 decimal)
   - `format_value($metric_avg, $metric.unit)` with a multi-column variable.
   - `format_value($cell.selected.bytes, 'bytes')` with a selected row.
   - `format_value($cell[0].duration_ns, 'nanoseconds')` — BigInt arg path.
   - Unknown function name `foo(1,2)` passed through unchanged, no warning.
   - Unresolved macro arg `format_value($missing, 'bytes')` — text left as-is **and** a warning emitted naming `$missing`.
   - Unresolved selection arg `format_value($cell.selected.bytes, 'bytes')` with no selection — text restored to original source (not partially substituted by pass 2, which would otherwise produce `format_value(, 'bytes')` because `substituteMacrosImpl` resolves missing `$cell.selected.col` to `''`) **and** a warning emitted. This is the regression case the sentinel protects against.
   - String literals containing commas: `format_value($x, 'GB, please')`.
   - Mixed: `format_value($x, 'bytes') extra $y` — first replaced, second substituted.
   - Warnings are deduplicated when the same unresolved arg appears in multiple calls.

### Phase 3 — Wire into Map

7. `components/map/EventDetailPanel.tsx:59`: swap `substituteMacros` → `evaluateTemplate`. Destructure `{ text, warnings }`; render `text` as Markdown and render `warnings` (if any) in a banner above the body. **Side effect:** the current call uses the SQL-escape path (doubles single quotes) on Markdown output; `evaluateTemplate` runs `substituteMacrosRaw` instead, so single quotes in Map templates are no longer doubled. Default template has no quotes, so this only affects user-customized templates. The regression test for this behavior change lives under *Testing Strategy* below.
7a. Create `components/TemplateWarningBanner.tsx` (or similar shared location) — a small component that renders an amber-bordered list of warning strings, hidden when the list is empty. Used by both the Map panel and the Table renderer.
8. Update `DEFAULT_MAP_DETAIL_TEMPLATE` in `lib/screen-renderers/notebook-utils.ts:157` — keep current default unchanged (no implicit function calls).
9. Manual test: notebook with a Map cell whose query returns a `bytes` column; template uses `format_value($bytes, 'bytes')`; verify adaptive output in the detail panel.

### Phase 4 — Wire into Table format overrides

10. Refactor `OverrideCell` to run `evaluateFunctionCalls` *before* the existing `expandVariableMacros` → `expandCellSelectionMacros` → `expandRowMacros` chain — otherwise those passes would stringify `$variable` / `$cell.selected.col` / `$row.col` references inside function-call args before the evaluator can read them as raw values. Extend both `resolveMacroRaw` and `evaluateFunctionCalls` to accept an optional `row?: Record<string, unknown>` param so `$row.col` and `$row["col"]` resolve to the raw Arrow value (a bare `row[col]` lookup — no `columnTypes` needed; the legacy `expandRowMacros` chain still owns RFC3339 stringification for `$row.*` references that appear outside function calls). Keep the legacy expansion chain to handle macros that appear outside function calls in the same template. After all passes complete, call the shared `restoreSentinels(text, sentinels)` helper exported from `notebook-utils.ts` so the Table path reuses the same restore step as `evaluateTemplate` instead of inlining the loop.
10a. Collect warnings across all rendered rows in the table (deduplicated) and surface them as a single `<TemplateWarningBanner>` above the table body. **Mechanism:** add a pre-render pass in `TableBody` (`table-utils.tsx:452`) that, before mapping rows to JSX, iterates `data.numRows × overrideMap` and calls `evaluateFunctionCalls(format, ..., row)` once per (row, override) tuple, accumulating warnings into a `Set<string>` keyed by warning text. Memoize the pass on `[data, overrideMap, variables, timeRange, cellResults, cellSelections]` so it doesn't rerun on unrelated re-renders. Hoist the warning set up to `TableRenderer` via a new `onWarnings?: (warnings: string[]) => void` callback on `TableBody`, or by returning the set from a small helper hook (`useOverrideWarnings`) the parent owns. `OverrideCell` keeps doing its own per-cell evaluation for the rendered output — the pre-render pass is purely for aggregation, and a `Map<(format, rowIdx), Result>` memo can be threaded through to avoid double work if profiling shows it matters. Per-row banners would be noisy; one aggregate banner per table is enough.
11. Add tests at `lib/screen-renderers/__tests__/table-utils.test.tsx` covering: (a) a column override that calls `format_value($row.bytes, 'bytes')` rendering adaptively, and (b) a column override with an unresolved arg (e.g., `format_value($cell.selected.missing, 'bytes')` with no selection) restoring the original source text and emitting a warning.

### Phase 5 — Documentation

12. Update `mkdocs/docs/web-app/notebooks/variables.md`:
    - Add a *Template Functions* subsection under *SQL Macro Substitution* describing the v1 surface and the `format_value(value, unit)` signature with a few examples.
    - Note the unit vocabulary (point to `lib/units.ts` aliases).
    - Note SQL queries do **not** support function calls in v1.

### Phase 6 — Checks

13. From `analytics-web-app/`: `yarn lint`, `yarn type-check`, `yarn test`.

## Files to Modify

| File | Change |
|---|---|
| `analytics-web-app/src/lib/format-value.ts` *(new)* | Shared `formatValueWithUnit` / `formatValueWithScale` |
| `analytics-web-app/src/lib/__tests__/format-value.test.ts` *(new)* | Unit tests for the shared formatter |
| `analytics-web-app/src/lib/template-functions.ts` *(new)* | `TEMPLATE_FUNCTIONS` registry + `format_value` impl |
| `analytics-web-app/src/components/XYChart.tsx` | Delete local `formatValue` / `formatStatValue`; import from shared module |
| `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` | Add `parseFunctionCalls`, `resolveMacroRaw`, `evaluateFunctionCalls`, `evaluateTemplate` |
| `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts` | Tests for `evaluateTemplate` and `format_value` |
| `analytics-web-app/src/components/map/EventDetailPanel.tsx` | Swap `substituteMacros` → `evaluateTemplate`; render warning banner |
| `analytics-web-app/src/components/TemplateWarningBanner.tsx` *(new)* | Reusable warning-banner component used by Map panel and Table renderer |
| `analytics-web-app/src/lib/screen-renderers/table-utils.tsx` | Run function-call evaluator before `expandRowMacros`; extend row-aware resolution; aggregate warnings and render a single banner above the table body |
| `analytics-web-app/src/lib/screen-renderers/__tests__/table-utils.test.tsx` | Test for `format_value($row.col, 'unit')` |
| `mkdocs/docs/web-app/notebooks/variables.md` | Document template functions |

## Trade-offs

### Sibling `evaluateTemplate` vs. extending `substituteMacros`

**Chosen: sibling function.** The issue explicitly leaves both open. A sibling keeps the SQL path frozen (no risk to existing query templates) and avoids paying the function-call regex cost for every SQL substitution — far more frequent than Markdown rendering. The trade-off is a small amount of surface duplication (two entry points calling the same impl), but the impl itself is shared via `substituteMacrosRaw`.

### Function-call grammar vs. suffix pipe

The issue rules this out at length (composability, type preservation, extensibility). This plan inherits that conclusion.

### Registry vs. switch statement

**Chosen: registry.** Adding `format_duration`, `round`, `concat`, `clamp` later should be one entry in a table, not an evaluator edit. Cost is negligible — one indirection.

### Raw BigInt → number coercion in `format_value`

**Chosen: coerce to `number` in the function impl.** `getAdaptiveSizeUnit` operates on `number`. Values up to `2^53` (≈9 PB in bytes) fit exactly; larger values lose low-order bits but remain accurate to ~15 significant digits, which is fine for adaptive scaling that displays 3 digits. Future work could add a BigInt-aware byte formatter if multi-petabyte precision is ever needed.

### Row column precision (Map)

`$cell[N].col` and `$cell.selected.col` are lossless on every surface: `resolveMacroRaw` reads from `cellResults[name].get(idx)[col]` and `cellSelections[name][col]` directly, returning the Arrow primitive (BigInt for i64, number for f64).

`$row.col` is lossless for **Table** because Phase 4 extends `resolveMacroRaw` with a `row` argument and `OverrideCell` already receives the raw `row: Record<string, unknown>`.

The one path that **isn't** lossless is `$x` (row column merged into vars) in **Map** templates: `materializeRow` in `components/map/overlay.ts:565` produces `Row = Record<string, string>` (see `overlay.ts:16`) — every cell goes through `formatArrowValue`, so by the time `EventDetailPanel` merges row into variables, values are strings. For `format_value` on such an arg, the evaluator reads a string and `Number()`-coerces it. Round-trip is exact for `number` and exact for `bigint` decimal stringification within the safe-integer range; it loses low-order bits for i64 values above 2^53.

That's adequate for v1 because adaptive scaling displays ~3 significant digits — a 9 PB+ byte count formatted as "3.4 GB" doesn't notice the lost bits. If lossless byte handling for >2^53 ever matters here, a follow-up can thread the raw Arrow row alongside the stringified `Row` to `EventDetailPanel`. Not worth doing speculatively.

### Phase 4 row access

`OverrideCell` already receives the raw `row: Record<string, unknown>`. The new function-call evaluator pass runs *before* the legacy `expandVariableMacros` → `expandCellSelectionMacros` → `expandRowMacros` chain, with `resolveMacroRaw` extended to accept an optional `row` argument — so `$row.col` and `$row["col"]` inside a function-call arg resolve to the raw Arrow value before `Number()` coercion, instead of the stringified form `expandRowMacros` would produce. The legacy chain still runs afterward to handle `$row.*` (and `$variable`, `$cell.selected`) macros that appear outside function calls in the same template; that chain keeps the existing `columnTypes` plumbing for its own RFC3339 stringification path.

### Quote-escape rules in v1

**No backslash escapes.** A unit identifier is at most 10 chars (`'gigabytes'`); supporting `'\\n'` invites parser complexity for no real use case. If a template ever needs a literal quote, switch the outer quote. Document this in the docs page.

## Documentation

`mkdocs/docs/web-app/notebooks/variables.md` — add a *Template Functions* subsection under *SQL Macro Substitution*:

- v1 functions: `format_value(value, unit)`.
- Where it works: Markdown templates (Map detail panel, table column overrides). **Not** SQL queries.
- Unit vocabulary: same aliases the chart understands (point readers to seconds/ms/bytes/percent/etc.).
- Mention that args may be macros (resolved before the function runs) or string literals in single or double quotes.

No changes needed to `cell-types.md` or `execution.md`.

## Testing Strategy

### Unit tests

- `lib/__tests__/format-value.test.ts`: every unit branch (time / size / bit / percent / degrees / boolean / unitless fallback). Identical input/output to today's chart behavior — these tests double as regression coverage for the chart refactor.
- `lib/screen-renderers/__tests__/notebook-utils.test.ts`:
  - Successful function calls with each macro shape (`$variable`, `$variable.column`, `$cell[N].column`, `$cell.selected.column`).
  - String literal args, mixed macro+literal, both quote styles.
  - Pass-through for unknown function names.
  - Pass-through for unresolved macro args (preserve original text).
  - BigInt arg (timestamp column → seconds unit).
  - Multiple function calls in one template.
  - Function call followed by normal `$variable` substitution outside it.
- `lib/screen-renderers/__tests__/table-utils.test.tsx`: column override with `format_value($row.col, 'bytes')` renders adaptive text.
- `lib/screen-renderers/__tests__/notebook-utils.test.ts`: regression for the Map quote-escape behavior change. `evaluateTemplate("msg: $search", { search: "it's working" }, …)` must produce `"msg: it's working"` (single quotes preserved, **not** doubled to `it''s`). Pins the consequence of Phase 3 step 7 swapping `substituteMacros` → `evaluateTemplate` so the SQL escape never silently leaks back in.

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
