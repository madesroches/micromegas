# Unify Macro Lookup Behind a Shared `resolveMacro(span, ctx)` Plan

## Issue Reference
- [#1088](https://github.com/madesroches/micromegas/issues/1088) — Unify template/SQL macro lookup behind a shared `resolveMacro(span, ctx)`
- Follow-up to [#1086](https://github.com/madesroches/micromegas/issues/1086) (`format_value` template functions) — see `tasks/completed/1086_template_function_calls_plan.md`.

## Overview

Two macro engines currently own parallel copies of the same macro-shape lookup logic. Extract a single private helper `resolveMacro(span, ctx)` that, given an *already-parsed* macro shape, returns its raw value, a `resolved` flag, and the source `dataType`. Both engines keep their own *parsing* strategy (regex sweep vs. left-to-right walker) but route every value lookup through `resolveMacro`. This removes the drift risk: a future change to a macro shape (new built-in, precedence tweak, null-cell corner case) is made once. **No functional change** — existing tests pass unchanged.

## Current State

The issue text describes the macro engine as living in `notebook-utils.ts` with a `resolveMacroRaw` helper inside `evaluateTemplate`. That predates the refactor that shipped with #1086: `notebook-utils.ts:5-9` documents that the macro engine is now split across two sibling modules, and there is no standalone `resolveMacroRaw` — lookup was folded directly into the walker's `tryParseMacro`. The real duplication today is between these two:

### Engine 1 — regex sweep (`macro-substitution.ts`)

`substituteMacrosImpl` (`macro-substitution.ts:86-146`) runs a fixed sequence of `String.replace` passes, one per macro shape, each with an inline lookup in its callback:

| Shape | Lookup site | Unresolved behavior |
|---|---|---|
| `$from` / `$to` | `:97-98` (plain global replace) | always resolved (SQL path always has `timeRange`) |
| `$cell[N].col` | `:101-110` | returns `match` (leave source) on missing table / OOB row / null cell |
| `$cell.selected.col` | `:113-121` | returns **`''`** (empty) on missing selection / null cell |
| `$variable.col` | `:124-131` | returns `match` on missing var / string var / missing col |
| `$variable` | `:135-143` (loop over known vars, sorted by name length desc) | only iterates known names → always resolves |

Each resolved value is run through `formatArrowValue` (`:25-31`, timestamps → RFC3339) then an `escape` callback. Two public entry points wrap the impl: `substituteMacros` (SQL escape, single-quote doubling, `:61-69`) and `substituteMacrosRaw` (identity escape, `:76-84`).

### Engine 2 — single-pass walker (`template-evaluator.ts`)

`tryParseMacro` (`template-evaluator.ts:77-219`) parses *and* looks up a macro at a given position, returning a `MacroMatch { source, end, shape, value, dataType }`. It recognizes the same shapes plus two the SQL path lacks (`$row.col` / `$row["col"]` at `:124-140`/`:171-182`, and bare `$col`-from-row when `ctx.bareColumnsFromRow` at `:206-211`). Its results feed two emission contexts:

- **Function-arg context** (`tryParseCallArgs:254-269`): pushes the **raw** `value` to the registry function (with a time-type stringification guard at `:264-266` so a BigInt epoch count doesn't lose precision); records unresolved shapes.
- **Naked-macro context** (`evaluateTemplate:377-392`): emits `formatArrowValue(value, dataType)` when resolved, else copies the source verbatim and emits a warning.

`EvaluateTemplateCtx` (`template-evaluator.ts:23-40`) already carries the full superset of context every shape needs: `variables`, `timeRange?`, `cellResults`, `cellSelections`, `row?`, `columnTypes?`, `bareColumnsFromRow?`.

### The four call sites (issue framing)

The issue counts four call sites; they are the **two engines × emission behaviors**:
1. `substituteMacros` → format → SQL-escape → emit.
2. `substituteMacrosRaw` → format → emit verbatim.
3. `evaluateTemplate` function-arg context → emit raw `value` to registry.
4. `evaluateTemplate` naked-macro context → format → emit verbatim.

The lookup bodies behind (1)+(2) live in `substituteMacrosImpl`; behind (3)+(4) in `tryParseMacro`. Unifying means extracting one lookup these all share.

### Tests

`analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts` (1079 lines) covers both engines: `substituteMacros` (`:25`), cell-ref / selected-ref substitution (`:307`, `:720`), and `evaluateTemplate` — `format_value calls` (`:890`), `macro behavior` (`:1001`), `bareColumnsFromRow` (`:1022`). These exercise every call site and must pass unchanged.

## Design

### Parse / lookup split

Keep both parsers. Introduce a value-lookup layer they share:

```
regex sweep ─┐                            ┌─ format + SQL-escape  (substituteMacros)
             ├─► MacroSpan ─► resolveMacro ┼─ format + identity    (substituteMacrosRaw)
walker      ─┘                            ├─ raw value             (template fn-arg)
                                          └─ format                (template naked)
```

`MacroSpan` is a parsed shape, independent of *how* it was found. `resolveMacro` does the lookup; callers own formatting/escaping/emission.

### New module — `macro-resolve.ts`

```ts
import type { Table, DataType } from 'apache-arrow'
import type { VariableValue } from './notebook-types'

/** A parsed macro shape, independent of the engine that parsed it. */
export type MacroSpan =
  | { kind: 'time'; which: 'from' | 'to' }
  | { kind: 'cellRow'; cell: string; rowIdx: number; col: string }
  | { kind: 'selected'; cell: string; col: string }
  | { kind: 'rowCol'; col: string }          // $row.col and $row["col"]
  | { kind: 'varCol'; name: string; col: string }
  | { kind: 'var'; name: string }            // $variable, or bare $col when bareColumnsFromRow

/** Everything any macro shape might need to resolve. Superset of the old
 *  EvaluateTemplateCtx; the SQL path supplies only the first four fields. */
export interface ResolveCtx {
  variables: Record<string, VariableValue>
  timeRange?: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
  row?: Record<string, unknown>
  columnTypes?: Map<string, DataType>
  bareColumnsFromRow?: boolean
}

export interface ResolvedMacro {
  /** Raw JS/Arrow value when resolved; undefined otherwise. */
  value: unknown
  resolved: boolean
  /** Arrow DataType of the value's source column, when known. Lets callers
   *  pick the right formatter (RFC3339 for timestamps) and detect time types. */
  dataType?: DataType
}

export function resolveMacro(span: MacroSpan, ctx: ResolveCtx): ResolvedMacro
```

`EvaluateTemplateCtx` becomes an alias of (or `extends`) `ResolveCtx`; its existing per-field doc comments move to `ResolveCtx`. `EvaluateTemplateResult` and the walker stay in `template-evaluator.ts`.

### `resolveMacro` lookup table

Each branch reproduces the *union* of both current implementations exactly (they already agree on lookup; they only differ in how each caller treats `resolved: false`, which moves to the callers).

| `span.kind` | Resolves when | `value` | `dataType` |
|---|---|---|---|
| `time` | `ctx.timeRange` set | `which === 'from' ? begin : end` | — |
| `cellRow` | table exists, `rowIdx < numRows`, cell non-null | `table.get(rowIdx)[col]` | field type |
| `selected` | `cellSelections[cell]` exists, value non-null | `selection[col]` | `cellResults[cell]` field type |
| `rowCol` | `ctx.row` set, value non-null | `ctx.row[col]` | `ctx.columnTypes?.get(col)` |
| `varCol` | var exists, is multi-column, col present | `variable[col]` (string) | — |
| `var` | (see precedence below) | — | — |

`var` precedence (mirrors `tryParseMacro:206-218`):
1. If `ctx.bareColumnsFromRow && ctx.row` and `row[name]` is non-null → `{ value: row[name], resolved: true, dataType: columnTypes?.get(name) }`.
2. Else `variables[name]`: string → that string; multi-column → `getVariableString(variable)`; undefined → `resolved: false`.

Crucially, `resolveMacro` does **not** decide what unresolved looks like in the output (`''` vs. source vs. warning) and does **not** escape or format — those stay with the callers, so SQL vs. template behavior is preserved verbatim.

### Caller 1+2 — `substituteMacrosImpl` (`macro-substitution.ts`)

Keep the regex passes; each callback builds a `MacroSpan`, calls `resolveMacro`, and applies the same format+escape it does today. A `ResolveCtx` is built once from the four params (`row`/`columnTypes`/`bareColumnsFromRow` omitted).

```ts
const emit = (r: ResolvedMacro) => escape(formatArrowValue(r.value, r.dataType))

// $cell[N].col
result = result.replace(cellRefRegex(), (match, cell, rowIdxStr, col) => {
  const r = resolveMacro({ kind: 'cellRow', cell, rowIdx: parseInt(rowIdxStr, 10), col }, ctx)
  return r.resolved ? emit(r) : match              // unchanged: leave source
})

// $cell.selected.col
result = result.replace(selectedRefRegex(), (_m, cell, col) => {
  const r = resolveMacro({ kind: 'selected', cell, col }, ctx)
  return r.resolved ? emit(r) : ''                 // unchanged: empty on unresolved
})

// $variable.col
result = result.replace(dottedVarRegex(), (match, name, col) => {
  const r = resolveMacro({ kind: 'varCol', name, col }, ctx)
  return r.resolved ? emit(r) : match              // unchanged: leave source
})
```

- `$from`/`$to`: route through `resolveMacro({ kind: 'time', which })` for single-source-of-truth, but the emitted text is identical (`escape(begin)` since `formatArrowValue(string, undefined) === String(string)`).
- `$variable` loop (`:135-143`): keep the sorted-by-name-length parsing (a regex-matching concern). Inside each iteration call `resolveMacro({ kind: 'var', name }, ctx)` for the value instead of the inline `typeof` branch.

**Equivalence note:** for `varCol` and `var`, the resolved value is already a `string`, and `escape(formatArrowValue(value, undefined))` collapses to `escape(value)` — byte-identical to the current inline `escape(colValue)` / `escape(value)`. So consolidating every branch onto one `emit` helper is safe.

### Caller 3+4 — walker (`template-evaluator.ts`)

`tryParseMacro` keeps doing the parsing it does now (it produces `source`, `end`, `shape`), but instead of inlining the lookup it builds the matching `MacroSpan`, calls `resolveMacro(span, ctx)`, and fills `MacroMatch.value` / `MacroMatch.dataType` from the result. The two emission sites are unchanged:

- Function-arg (`tryParseCallArgs:254-269`): keeps its time-type stringification guard (`isTimeType(dataType)` → `formatArrowValue`), reads `m.value` / `m.dataType` from the now-`resolveMacro`-backed `MacroMatch`, and treats `value === undefined` as unresolved (equivalently `!resolved`). The `$row["col"]` and `$cell[N]` digit-vs-string parsing branches (`:97-143`) stay in `tryParseMacro`; only their value-lookup bodies move into `resolveMacro`.
- Naked (`evaluateTemplate:377-392`): unchanged — resolved → `formatArrowValue`, else source + warning.

### Where lookup *parsing detail* stays vs. moves

Stays with the parser (engine-specific): the `$variable` name-length sort, the `$cell[N]` digit-vs-`$row["col"]` string disambiguation, the `$row.col`-before-`$variable.col` precedence in the walker, the `\b(?![.[])` lookaheads. Moves into `resolveMacro`: every table/selection/variable/row value fetch and null check.

## Implementation Steps

1. **Create `analytics-web-app/src/lib/screen-renderers/macro-resolve.ts`** with `MacroSpan`, `ResolveCtx`, `ResolvedMacro`, and `resolveMacro`. Import `getVariableString` from `./notebook-types`. Port each lookup branch from `substituteMacrosImpl` / `tryParseMacro` per the table above. No `formatArrowValue`/escape inside — raw values only. Keep it un-re-exported from `notebook-utils.ts` (acceptance criterion: no new public exports).

2. **Refactor `macro-substitution.ts`**: build a `ResolveCtx` from the impl's params; replace each `.replace` callback body and the `$variable` loop body with `resolveMacro` + the shared `emit` helper. Preserve the `''`-on-unresolved for `$cell.selected.col` and the leave-`match` for the others. Remove the now-redundant inline lookup logic. `formatArrowValue` stays here (still publicly re-exported, still imported by `template-evaluator.ts`).

3. **Refactor `template-evaluator.ts`**: make `EvaluateTemplateCtx` an alias of / extend `ResolveCtx` (move the field doc comments). In `tryParseMacro`, after parsing each shape, construct the `MacroSpan` and call `resolveMacro(span, ctx)`; populate `MacroMatch.value` / `.dataType` from the result. Leave `tryParseCallArgs` and the main-loop emission unchanged.

4. **Run the suite** — existing tests in `__tests__/notebook-utils.test.ts` must pass unchanged (the proof of no behavior change).

5. **Add focused `resolveMacro` unit tests** (see Testing Strategy) covering each `MacroSpan` kind × resolved/unresolved, importing directly from `./macro-resolve`.

6. **Checks**: from `analytics-web-app/`: `yarn lint`, `yarn type-check`, `yarn test`.

## Files to Modify

| File | Change |
|---|---|
| `analytics-web-app/src/lib/screen-renderers/macro-resolve.ts` *(new)* | `MacroSpan`, `ResolveCtx`, `ResolvedMacro`, `resolveMacro` — the single private lookup |
| `analytics-web-app/src/lib/screen-renderers/macro-substitution.ts` | Route every regex-callback lookup through `resolveMacro`; build `ResolveCtx`; keep escape/format and unresolved behaviors |
| `analytics-web-app/src/lib/screen-renderers/template-evaluator.ts` | `EvaluateTemplateCtx` ≡ `ResolveCtx`; `tryParseMacro` builds a `MacroSpan` and calls `resolveMacro` |
| `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts` | Add a `resolveMacro` unit block; existing blocks unchanged |

No changes to `notebook-utils.ts` (public barrel), `template-functions.ts`, or any cell/component. No documentation changes (internal refactor, no user-visible surface).

## Trade-offs

- **One lookup, two parsers (chosen)** vs. unifying parsing too. The regex sweep and the walker have genuinely different parsing models (whole-string multi-pass vs. positional single-pass), and the walker's `$row`/bare-column shapes don't exist in SQL. Unifying *parsing* would be a much larger, behavior-risky change for little gain; the drift risk the issue calls out is in *lookup*, which this fully addresses.
- **`resolveMacro` returns raw value + `resolved` flag, leaves emission to callers (chosen)** vs. a formatter parameter. The four call sites format/escape/handle-unresolved differently (SQL escape, identity, raw-to-registry, RFC3339 + warning). Returning the raw value keeps each caller's distinct behavior intact and is exactly the seam the `format_value` precision path needs (raw BigInt to the registry).
- **New `macro-resolve.ts` module (chosen)** vs. placing `resolveMacro` in one existing module and importing across. A neutral third module avoids making either engine depend on the other and keeps the shared unit obviously shared. No import cycle: `macro-resolve` depends only on `notebook-types`; both engines depend on it.
- **Route `$from`/`$to` through `resolveMacro` too (chosen)** vs. leaving the trivial global replace inline. Marginally more code at the call site, but satisfies "one lookup function used by all four call sites" and keeps the macro grammar fully in one place. Output is provably identical.

## Testing Strategy

Primary guarantee is **existing tests pass unchanged** — this is a pure refactor. To pin the new shared unit directly, add a `describe('resolveMacro')` block in `__tests__/notebook-utils.test.ts` (or a sibling `macro-resolve.test.ts`) importing from `../macro-resolve`:

- `time`: `from`/`to` resolved when `timeRange` set; `resolved: false` when omitted.
- `cellRow`: resolved with correct `value` + `dataType`; unresolved on missing table, OOB row, null cell.
- `selected`: resolved with `dataType` from `cellResults`; unresolved on missing selection / null value. (Asserts `resolved: false`, not `''` — the `''` mapping is the SQL caller's job and is already covered by `substituteMacros` tests.)
- `rowCol`: resolved only when `ctx.row` set; `dataType` from `columnTypes`; unresolved on null/missing column.
- `varCol`: resolved for multi-column var; unresolved for string var, missing var, missing column.
- `var`: simple string; multi-column → `getVariableString`; `bareColumnsFromRow` makes `row[name]` win over a same-named variable; unresolved for unknown name.

Then confirm the four call-site behaviors are byte-for-byte preserved via the existing `substituteMacros`, cell/selected-ref, and `evaluateTemplate` suites (no edits expected). Acceptance is the full `yarn test` passing with only additions, never edits, to assertions.

## Open Questions

None. The issue scopes this as a no-behavior-change refactor with explicit acceptance criteria; the only deviation from the issue text is cosmetic — the code already moved out of `notebook-utils.ts` into the two sibling modules, so the shared helper lands in a new `macro-resolve.ts` (still private to the macro modules, not re-exported) rather than inside `notebook-utils.ts`.
