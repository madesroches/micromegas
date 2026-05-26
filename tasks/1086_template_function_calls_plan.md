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

`formatValueWithUnit` is the body of `formatStatValue` today: use `formatTimeValue` for time units, fall through to the size/bit/percent/degrees/boolean ladder. The size/bit/etc. ladder (today's `formatValue` body, minus the unused `adaptiveTimeUnit` and `abbreviated` parameters) lives as a private helper inside the module. Both parameters are dropped during the lift: `adaptiveTimeUnit` has no live caller in `XYChart.tsx` today (the chart's axis code at lines 625-635 and 761-769 formats inline; `formatValue`'s only caller is `formatStatValue`, which omits it), and `abbreviated` is read only inside the now-removed adaptive-time branch — the size/bit/percent/degrees/boolean ladder never references it. The chart imports `formatValueWithUnit` (replacing the existing `formatStatValue` invocations across `XYChart.tsx`); the template engine imports the same.

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
   *  emitted as naked `$row.col` macros outside function-call args. */
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
    try to parse a macro shape, in this precedence order:
      1. $from, $to                    (only when ctx.timeRange is set;
                                        otherwise treated as unresolved
                                        and left in place as source)
      2. $cell[N].col
      3. $cell.selected.col
      4. $row.col, $row["col"]         (only when ctx.row is set — matched
                                        BEFORE $variable.col so that a row
                                        column reference never gets
                                        shadowed by the $variable.col
                                        shape with varName='row')
      5. $variable.col
      6. $variable
    if a shape matches and resolves:
      copy formatArrowValue(value, dataType)     // RFC3339 for timestamps;
                                                 // dataType source per shape:
                                                 //   $cell[N].col, $cell.selected.col → cellResults[cell].schema.fields.find(f=>f.name===col)?.type
                                                 //   $row.col, $row["col"]            → ctx.columnTypes?.get(col)
                                                 //   $variable, $variable.col, $from, $to → undefined (value is already a string)
      advance past macro
    elif a shape matches but is unresolved:
      emit a warning naming the macro shape      // e.g. "$cell.selected.bytes is unresolved"
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

Listed in precedence order — the walker tries each shape in turn and stops at the first one whose syntax matches at the current position (regardless of whether the match resolves or stays unresolved):

| Macro shape | Returns |
|---|---|
| `$from`, `$to` | `string` (ISO range value) when `ctx.timeRange` is set; `undefined` otherwise (matches the current `OverrideCell` behavior of skipping from/to merge when its `timeRange` prop is absent) |
| `$cell[N].column` | the Arrow value (`bigint` for timestamps and i64s, `number` for floats, etc.) |
| `$cell.selected.column` | same as above, from `cellSelections[cell][column]` |
| `$row.col` / `$row["col"]` | the row's raw Arrow value — **only when `ctx.row` is set** (Table override path). On surfaces without a row, these shapes are not recognized and the walker skips them. **Matched BEFORE `$variable.col` so a row column reference is never shadowed by the dotted-variable shape with `varName='row'`** (which, with `variables['row']` typically undefined, would otherwise consume `$row.col` as a match-but-unresolved and emit a spurious warning). **A null or `undefined` cell value (or a missing column) is treated as unresolved** (returns `undefined`), matching the design's uniform "leave unresolved as source" rule and the parallel handling of null `$cell[N].col` in `substituteMacrosImpl:368`. |
| `$variable.column` | `string` (combobox column value) when `variables[var]` is a multi-column object; **`undefined`** when it's a simple string — matches `substituteMacrosImpl:392-394`, which returns `match` (leaves unresolved) for the simple-string case |
| `$variable` | `string` (or `getVariableString(value)` for multi-column) |
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
    // Reject empty strings explicitly — Number("") is 0 (finite), so without
    // this guard an empty-but-defined variable would silently render "0 B"
    // instead of surfacing as an unresolved-arg warning.
    if (rawValue === '') return undefined
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
| `components/map/EventDetailPanel.tsx:59` | `substituteMacros(...)` → `evaluateTemplate(template, { variables: mergedVars, timeRange, cellResults, cellSelections })`. Destructure `{ text, warnings }`; render `text` as Markdown and `<TemplateWarningBanner warnings={warnings} />` above the body. |
| `lib/screen-renderers/cells/MarkdownCell.tsx:21` | Same swap. Markdown cells share the Map detail-panel rationale: the SQL-escape pass is wrong for plain Markdown content (it doubles single quotes), and authors should be able to call `format_value` in cell bodies. |
| `lib/screen-renderers/table-utils.tsx:OverrideCell` | Replace the legacy `expandVariableMacros` → `expandCellSelectionMacros` → `expandRowMacros` chain with a single `evaluateTemplate(format, { variables, timeRange, cellResults, cellSelections, row, columnTypes })` call. The walker resolves `$row.col`, `$cell.selected.col`, `$variable`, `$cell[N].col`, and `$from/$to` all in one pass — and it natively handles `format_value(...)` calls. The existing `allVariables` `useMemo` (which merged `from`/`to` into `variables`) is dropped because the walker resolves those from `ctx.timeRange` directly. The legacy `expand*Macros` exports and their tests are deleted (`OverrideCell` was their sole production caller). Warnings are posted via `WarningReporterContext` (see §6a). |
| Table parents (`cells/TableCell.tsx`, `cells/TransposedTableCell.tsx`, `TableRenderer.tsx`) | All three host `OverrideCell` and need the same warning-collection wiring. Each calls `useColumnWarnings(<stable-overrides-source>)` (shared hook in `warning-reporter.tsx`), provides `WarningReporterContext`, and surfaces `<ColumnHeaderWarningIcon>` on the column label — passed as `SortHeader`'s new `trailingIcon` slot for the two header-based tables, rendered inline next to the row label `<td>` for the transposed layout. No dry-run pass, no row cap, no parent memoization preconditions. |

**Side effects of the Map/Markdown swaps:**

The four numbered items below are *text-rendering* differences: cases where the legacy macro engine produced one visible string and the walker produces another. Orthogonally to these, the walker introduces a **new diagnostic surface** at every swap site: any naked unresolved macro — including a plain unknown `$variable` (e.g. `Hello $missing`) — now emits a warning that the surface displays (banner above Map detail panels and Markdown cell bodies; column-header icon above the affected table column). The legacy engines had no warning equivalent. It is not numbered as a side-effect because the rendered text for these "plain unresolved variable" cases doesn't change (both old and new keep the macro as source), only the diagnostic surface is new.

1. *Quote escaping:* the previous `substituteMacros` path doubled single quotes (SQL escape) on Markdown output — a latent bug, since neither surface is SQL. After the swap, quotes render verbatim. Default templates have no quotes, so this only affects user-customized templates.
2. *Naked unresolved `$cell.selected.col`:* under `substituteMacrosImpl` an unresolved selection macro collapsed to `''` (line 378/380), so `"a $cell.selected.col b"` rendered as `"a  b"`. The walker leaves the original macro source in place instead (and emits a warning), so the same template now renders as `"a $cell.selected.col b"`. This is intentional — the walker treats unresolved macros uniformly — and matches the function-call-arg behavior described in §4. **The same change applies to the table override path**: `expandCellSelectionMacros` (`table-utils.tsx:209-232`) also returns `''` for unresolved selection today, so column override `format` strings that bare-name `$cell.selected.col` will switch from collapsing to source-preserving + warning when `OverrideCell` swaps to `evaluateTemplate`.
3. *Variable boundary regex in table overrides:* the legacy `expandVariableMacros` (`table-utils.tsx:159-172`) uses `\\$${name}\\b` with no `(?![.[])` negative lookahead. Because `.` and `[` are non-word, `$metric.unit` (with `metric` a simple-string variable) currently expands to `<metric-value>.unit`, and `$metric[0]` currently expands to `<metric-value>[0]`. After the swap, the walker recognizes `$metric.unit` as the dotted-variable shape; per existing `substituteMacrosImpl` semantics a string-typed `metric` leaves the macro unresolved, so the same template now renders as `$metric.unit` (and emits a warning). The `$metric[0]` case fails to match any walker shape (no `[N].col` suffix) and is left as source. Net effect: any override that chains a literal `.suffix` or `[N]`-style suffix onto a simple-string variable switches from partial substitution to source-preserving + warning.
4. *Null `$row.col` (and `$row["col"]`) in table overrides:* the legacy `expandRowMacros` calls `formatValueForUrl` (`table-utils.tsx:50-65`), which returns `''` when `row[col]` is `null`/`undefined` or the column is missing. After the swap, per §4 above the walker treats those cases as unresolved and copies the original `$row.col` source verbatim (and emits one warning per row+column tuple, deduplicated by the `WarningReporterContext` reducer so the column-header icon shows one distinct entry, not N). Net behavior: null cells in a custom-formatted column switch from rendering as empty cells to rendering as the literal macro text with the column header marked. Affects override templates that ever encounter a null/missing column value.

The testing strategy below pins these behavior changes.

### 6a. Warning surface

`evaluateTemplate` returns `{ text, warnings }`:
- `text` is the rendered output; unresolved calls and unresolved macros appear as their original source.
- `warnings` is `string[]` — one entry per problem in source order, deduplicated within a single `evaluateTemplate` call.

The shape of the warning surface differs by caller:

**Single-template surfaces** (`EventDetailPanel`, `MarkdownCell`) — one `evaluateTemplate` call per render. The caller destructures both, renders `text` as Markdown and `<TemplateWarningBanner warnings={warnings} />` above the body. Empty `warnings` hides the banner. The banner is a thin reusable component (amber-bordered list of warning strings).

**Table override surface** (`OverrideCell`, called once per row per overridden column) — warnings collect during normal render via a React context, keyed by column. The collected state is a `Map<string, Set<string>>` (column name → distinct warning strings) held in `useState` at the table parent (`TableCell` for notebook tables, `TableRenderer` for screen tables, `TransposedTableCell` for transposed tables — same parent treatment for all three, since each renders `OverrideCell`). All three sites need the same `useState` + `useCallback`-stable reporter + reset-on-override-change `useEffect` triple, so the plan extracts that triple into a single `useColumnWarnings` hook in `warning-reporter.tsx`:

```ts
// warning-reporter.tsx
export function useColumnWarnings(overridesSource: unknown): {
  columnWarnings: Map<string, Set<string>>
  reportWarning: (columnKey: string, warning: string) => void
} {
  const [columnWarnings, setColumnWarnings] = useState<Map<string, Set<string>>>(new Map())
  const reportWarning = useCallback((columnKey: string, warning: string) => {
    setColumnWarnings(prev => {
      const existing = prev.get(columnKey)
      if (existing?.has(warning)) return prev   // dedup; no state churn
      const next = new Map(prev)
      next.set(columnKey, new Set(existing).add(warning))
      return next
    })
  }, [])
  // Reset when the override list changes. Callers MUST pass the *stable source* —
  // the raw `options?.overrides` (TableCell / TransposedTableCell) or
  // `tableConfig.overrides` (TableRenderer) — NOT a locally-destructured form
  // like `(options?.overrides as ColumnOverride[] | undefined) || []`, whose
  // `|| []` fallback produces a fresh array reference on every render when no
  // overrides are configured; that would re-fire the effect every render,
  // schedule a fresh `new Map()`, and trip React's "Too many re-renders" guard.
  useEffect(() => { setColumnWarnings(new Map()) }, [overridesSource])
  return { columnWarnings, reportWarning }
}
```

Table-parent wiring sketch (same shape for `TableCell`, `TableRenderer`, `TransposedTableCell` — each passes its own stable-source override prop):

```tsx
// Table parent
const { columnWarnings, reportWarning } = useColumnWarnings(options?.overrides)

return (
  <WarningReporterContext.Provider value={reportWarning}>
    <table>
      <thead><tr>
        {visibleColumns.map((col) => {
          const colWarnings = columnWarnings.get(col.name)
          return (
            <SortHeader
              key={col.name}
              columnName={col.name}
              trailingIcon={colWarnings?.size
                ? <ColumnHeaderWarningIcon warnings={[...colWarnings]} />
                : null}
              {...sortProps}
            >
              {col.name}
            </SortHeader>
          )
        })}
      </tr></thead>
      <TableBody … />
    </table>
  </WarningReporterContext.Provider>
)

// OverrideCell — gains a `columnName: string` prop for keying
const reporter = useContext(WarningReporterContext)
const { text, warnings } = evaluateTemplate(format, ctx)
const warningKey = warnings.join('\n')
useEffect(() => {
  if (!reporter || warnings.length === 0) return
  warnings.forEach(w => reporter(columnName, w))
  // `warnings` is intentionally omitted from deps — the array reference is
  // fresh on every render (new `evaluateTemplate` call), so listing it would
  // re-fire the effect each render. `warningKey` is the stable string
  // projection that gates re-runs to actual content changes.
  // eslint-disable-next-line react-hooks/exhaustive-deps
}, [warningKey, columnName, reporter])
return <Markdown>{text}</Markdown>
```

Why this shape works without parent memoization, row caps, or a dry-run pass:
- `reportWarning` is referentially stable (`useCallback` with `[]` deps). The reducer-style setState dedupes — once `(column, warning)` is reported, subsequent reports are no-ops at the parent. The cell's `useEffect` is free to fire on each render where `warningKey` changes; only first-time pairs cause a parent re-render.
- Coverage is naturally bounded by what `TableBody` renders. In notebook tables this is `slicedData` (the paginated page — ~50 rows × N overrides per page). In screen tables (`TableRenderer`) the full unsliced result renders, but dedup means the parent only re-renders for *new* warnings, not for every row that contributes the same one. A 10k-row × 5-override worst case still produces at most one re-render per distinct warning string.
- Warnings are sticky across pagination: once a column is flagged, it keeps its icon until the override format is edited (the reset effect on `[overrides]`). That matches the failure mode — schema/typo errors are systematic, not row-specific — and tells the author "this column has issues" regardless of which page they're viewing.

**`SortHeader` change** (`table-utils.tsx:320-409`): add an optional `trailingIcon?: React.ReactNode` prop, rendered inside the existing `<div className="flex items-center gap-1">` as a sibling of `<span className="truncate">{children}</span>` (i.e., *outside* the truncate span). Placing the icon outside the truncate span matters: with the icon inside the truncate span, `overflow:hidden; white-space:nowrap; text-overflow:ellipsis` would clip the icon exactly when the column name is long enough to truncate — i.e., when the warning is most needed. As a sibling of the truncate span, the flex container keeps the icon visible regardless of label length. Existing callers that don't pass `trailingIcon` are unaffected.

`<ColumnHeaderWarningIcon warnings={string[]}>` (new): an amber-tinted `<AlertTriangle>` (lucide-react, already a project dep), sized to match the sort indicators, with `title={warnings.join('\n')}` for hover detail. Click-to-open-edit-panel is a clean follow-up — same `columnWarnings` state can be read from anywhere that knows the column name.

**Transposed-table layout note:** `TransposedTableCell` (`cells/TransposedTableCell.tsx:31`) renders rows that correspond to the source query's *columns* (line 105-107 renders the row name in a `<td>`, not a `<th>`/`SortHeader`). The transposed table has no `SortHeader` to attach `trailingIcon` to. Instead, the wiring renders `<ColumnHeaderWarningIcon>` inline next to `{row.name}` inside the row-label `<td>` (e.g., wrapped in a `flex items-center gap-1` span). Same `useColumnWarnings(options?.overrides)` hook, same provider, same `OverrideCell` warning-posting path — only the icon's placement differs.

`<TemplateWarningBanner>` is used by Map detail panel and Markdown cell only.

## Implementation Steps

### Phase 1 — Shared formatter

1. Create `analytics-web-app/src/lib/format-value.ts` with `formatValueWithUnit` (single export, per *Design §1*). Body lifted from `XYChart.tsx:57-101`; the size/bit/etc. ladder lives as a private helper in the same file.
2. Update `components/XYChart.tsx` to import the new helpers and delete the local `formatValue` / `formatStatValue`. No behavior change.
3. Add unit tests at `lib/__tests__/format-value.test.ts` covering: time units (ns / µs / ms / s / min / h / d), size units (bytes / KB / MB / GB / TB), bit units, `percent`, `degrees`, `boolean`, and the unitless fallback.

### Phase 2 — Template evaluator

4. Create `lib/template-functions.ts` with the `TEMPLATE_FUNCTIONS` registry and `format_value` implementation.
5. In `lib/screen-renderers/notebook-utils.ts`:
   - Add `resolveMacroRaw(macroSpan, ctx): unknown` per *Design §4*. Reuses the same regexes as `substituteMacrosImpl` for parsing a macro shape but returns the raw value (or `undefined` when unresolved — including the `$cell.selected.col`-with-no-selection case, which is *not* mapped to `''` here).
   - Add `evaluateTemplate(text, ctx): { text: string; warnings: string[] }` as the single-pass walker described in *Design §2*. The walker has three branches per character position: (a) identifier-in-registry followed by `(` → function call; (b) `$` → macro; (c) literal copy. Per-call outcomes (function-call branch):
     - **Parse abort** (e.g., missing `)`, malformed arg): walker falls back to literal copy at the current position; not treated as a call.
     - **Unknown function name**: function-call branch doesn't fire (registry miss). Walker copies the identifier as text; any `$macro` inside the following parens is resolved normally when the walker reaches it.
     - **Unresolved macro arg** (any arg resolves to `undefined`): copy the original call span verbatim; emit one warning per unresolved arg naming the macro shape.
     - **Registry function returns `undefined`** (wrong arity, non-finite numeric coercion, etc.): copy the original call span verbatim; emit one warning naming the function and the reason (e.g., `"format_value: argument 1 is not a numeric value"`, `"format_value: expected 2 arguments, got N"`). Keeps user-visible output identical to the source instead of leaking the literal string `"undefined"`.
     - **Success**: copy the formatted return value.
   - Per-macro outcomes (`$` branch, mirrors the function-call branch's success/failure split):
     - **Shape matches and resolves**: copy `formatArrowValue(value, dataType)` per the §2 pseudocode's dataType-source table.
     - **Shape matches but is unresolved** (e.g., `$cell.selected.col` with no selection, `$row.col` for a null cell, `$variable.col` with a string-typed variable, `$missing`): copy the original macro source verbatim **and** emit a warning naming the macro shape. This is what §6 side-effects #2/#3/#4 and the step 15 test (e) rely on — without warning emission here, naked unresolved macros would silently appear as literal text with no diagnostic surface.
     - **No shape matches** (e.g., `$ ` with a space, bare `$` in user text): copy `$` as a literal and advance one character; no warning (this isn't a "problem" — the author wrote a literal `$`).
   - Warnings are deduplicated within a single `evaluateTemplate` call (a `Set<string>` collected during the walk, returned as `string[]` in insertion order). Function-call-branch warnings and macro-branch warnings share the same dedup set but use distinct prefixes by design — `"format_value: $missing is unresolved"` (function-call context) vs `"$missing is unresolved"` (naked) — so the same `$missing` referenced both ways produces two entries that convey different render-context information.
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
   - Naked unresolved selection (regression-pin): `evaluateTemplate("a $cell.selected.col b", { cellSelections: {}, … })` → text `"a $cell.selected.col b"` (macro left as-is, **not** collapsed to `"a  b"` like `substituteMacrosImpl` would) **and** `warnings` contains one entry naming `$cell.selected.col`. Pins the §6 side-effect #2 behavior change and the macro-branch warning-emission rule from step 5 above.
   - Quote-escape regression: `evaluateTemplate("msg: $search", { variables: { search: "it's working" }, … })` produces `"msg: it's working"` (single quotes preserved, **not** doubled). Pins the §6 side-effect #1 behavior change.

### Phase 3 — Wire into Map and Markdown cells

7. Create `components/TemplateWarningBanner.tsx` — a small component that renders an amber-bordered list of warning strings, hidden when the list is empty. Used by the Map panel and the Markdown cell.
8. `components/map/EventDetailPanel.tsx:59`: swap `substituteMacros` → `evaluateTemplate`. Destructure `{ text, warnings }`; render `text` as Markdown and render `<TemplateWarningBanner warnings={warnings} />` above the body.
9. `lib/screen-renderers/cells/MarkdownCell.tsx:21`: swap `substituteMacros` → `evaluateTemplate`. Same shape — destructure `{ text, warnings }`, render `<TemplateWarningBanner warnings={warnings} />` above the existing Markdown body.
10. Manual test: notebook with a Map cell whose query returns a `bytes` column; template uses `format_value($bytes, 'bytes')`; verify adaptive output in the detail panel. Add a Markdown cell with body `Size: format_value($total_bytes, 'bytes')` referencing a variable; verify the cell renders the adaptive output.

### Phase 4 — Wire into Table format overrides

11. **Add warning surface primitives** at `lib/screen-renderers/warning-reporter.tsx` (new file):
    - `WarningReporterContext: React.Context<((columnKey: string, warning: string) => void) | null>` (default `null`).
    - `useColumnWarnings(overridesSource: unknown)` hook — returns `{ columnWarnings, reportWarning }`. Encapsulates the `useState<Map<string, Set<string>>>` + `useCallback`-stable reducer-style reporter + `useEffect` reset-on-`overridesSource`-change triple. Single home for the "depend on the stable source, not the `|| []` fallback" footgun.
    - `<ColumnHeaderWarningIcon warnings={string[]}>`: amber-tinted `<AlertTriangle>` (lucide-react), sized `w-3.5 h-3.5`, with `title={warnings.join('\n')}`. Returns `null` if `warnings.length === 0` (defensive; callers gate on size already).
    - **Update `SortHeader`** (`table-utils.tsx:320-409`) to accept an optional `trailingIcon?: React.ReactNode` prop, rendered inside the existing `<div className="flex items-center gap-1">` *after* the truncate span (so the icon is never clipped by `truncate` on long column names). Existing callers that don't pass `trailingIcon` keep working unchanged.

12. **Refactor `OverrideCell`** (`table-utils.tsx:260-298`):
    - Add a required `columnName: string` prop so cells can post warnings keyed by their column.
    - Call `evaluateTemplate(format, { variables, timeRange, cellResults, cellSelections, row, columnTypes })` and render `text` as Markdown.
    - Drop the existing `allVariables` `useMemo` (lines 271-274) that merged `from`/`to` into `variables` — the walker resolves `$from`/`$to` natively from `ctx.timeRange`.
    - Consume `WarningReporterContext` via `useContext`. Post warnings via `useEffect` with a dep on `[warnings.join('\n'), columnName, reporter]` so the effect fires only when the warning set actually changes. Reporter is `null` outside a provider (no-op).
    - Update the call site at `table-utils.tsx:510` to pass `columnName={col.name}`.

13. **Delete legacy macro expansion functions** and their now-dead helpers: remove `expandVariableMacros`, `expandCellSelectionMacros`, `expandRowMacros` exports from `table-utils.tsx:159-232` and the corresponding standalone `describe` blocks in `__tests__/table-utils.test.tsx` (`describe('expandRowMacros')` at line 20, `describe('expandVariableMacros')` at 136, `describe('expandCellSelectionMacros')` at 173, `describe('expandRowMacros with timestamps')` at 613). Also delete the three private helpers that were only used by `expandRowMacros` and have no other callers in `src/`:
    - `formatValueForUrl` (`table-utils.tsx:50-65`)
    - `DOT_NOTATION_REGEX` (`table-utils.tsx:42`)
    - `BRACKET_NOTATION_REGEX` (`table-utils.tsx:45`)
    `OverrideCell` was the sole production caller of the `expand*` chain; the walker tests in `notebook-utils.test.ts` cover the same macro shapes (plus `$cell[N].col` and `$from`/`$to`). The kept validators (`extractMacroColumns`, `findUnknownMacros`, `validateFormatMacros`) continue to compile — `extractMacroColumns` re-creates its own regexes inline (lines 76, 82) rather than depending on the module-level constants.

    **Also delete the existing `OverrideCell` test blocks** (`describe('OverrideCell')` at `table-utils.test.tsx:421-562` and `describe('OverrideCell with hidden timestamp column')` at `:564-611`). Two reasons they can't stay as-is: (a) none of the existing `render(<OverrideCell ... />)` calls pass the new required `columnName` prop, so TS compilation breaks across ~13 sites; (b) several assertions test legacy collapse-to-empty behavior the plan deliberately replaces — `:467-473` expects `$row.missing` → empty href arg (now preserved source + warning, §6 #4), `:549-561` expects `$cell.selected.x`-with-no-selection → empty href arg (now preserved source + warning, §6 #2). The new test cases in step 15 (a)–(g) cover the same macro shapes against the new behavior, including the RFC3339 timestamp rendering path (case (a) uses `format_value($row.bytes, 'bytes')`, and a new case (h) below preserves the hidden-timestamp-column coverage):
    - (h) Hidden timestamp column with `allColumns` containing the type: `format` = `"Started: $row.start_time"`, `row.start_time` = a microsecond BigInt → renders as RFC3339. Preserves the `:564-611` test's intent against the new evaluator.

14. **Wire into table parents.** All three sites call `useColumnWarnings(<stable-overrides-source>)` (from step 11) and wrap their `<table>` in `<WarningReporterContext.Provider value={reportWarning}>`. They differ only in where they render `<ColumnHeaderWarningIcon>`:
    - **`cells/TableCell.tsx` (notebook table cell)** — call `useColumnWarnings(options?.overrides)`. **Pass the stable source `options?.overrides`, NOT the locally-destructured `overrides`** at `TableCell.tsx:34` (the local is `(options?.overrides as ColumnOverride[] | undefined) || []` and its `|| []` fallback yields a fresh array reference per render whenever no overrides are configured, which would re-fire the reset effect on every render and trip React's "Too many re-renders" guard — see the hook docstring in step 11). Wrap the existing `<table>` at lines 144-166 in the provider. On the `SortHeader` at lines 151-163, pass `trailingIcon={columnWarnings.get(col.name)?.size ? <ColumnHeaderWarningIcon warnings={[...columnWarnings.get(col.name)!]} /> : null}` (keep `{col.name}` as the children). No changes to `NotebookRenderer`.
    - **`TableRenderer.tsx` (screen-level table)** — call `useColumnWarnings(tableConfig.overrides)`. These hooks live at the top of the component body, not inside `renderContent()`, so the early returns at lines 366-372 don't violate rules of hooks. The `tableConfig.overrides` reference is config-stable across renders (replaced via spread on edits), so the dep is safe without further memoization. Wrap the `<table>` at lines 385-405 in the provider (the `reportWarning` closure works fine from `renderContent`). On the `SortHeader` at lines 389-400, pass `trailingIcon` the same way as `TableCell`. Even without notebook variables in scope, `format_value($row.col, 'bytes')` still applies in screen tables and can fail.
    - **`cells/TransposedTableCell.tsx` (transposed notebook table cell)** — also renders `OverrideCell` (`TransposedTableCell.tsx:111-120`), so it needs the same provider treatment. Call `useColumnWarnings(options?.overrides)` (same stable-source rule as `TableCell`). Wrap the `<table>` at lines 100-129 in the provider. Transposed tables have no `SortHeader` — the source-query columns are row labels rendered inside `<td>` at lines 105-107 (`{row.name}`). Replace `{row.name}` with a `<span className="flex items-center gap-1">{row.name}{colWarnings?.size ? <ColumnHeaderWarningIcon warnings={[...colWarnings]} /> : null}</span>` where `colWarnings = columnWarnings.get(row.name)`. Row labels are not truncated in the transposed layout (line 105 uses `whitespace-nowrap` without `overflow-hidden`), so the icon stays visible alongside the label. **Pass `columnName={row.name}` to the `OverrideCell` at `TransposedTableCell.tsx:112-120`** — this is the second production call site that step 12's required `columnName` prop addition affects (the first being `table-utils.tsx:510`).
    - **`cells/ReferenceTableCell.tsx`** — unchanged. No overrides today, no warnings to surface. If overrides are ever added, copy the same hook + provider wiring.

15. **Add tests** at `lib/screen-renderers/__tests__/table-utils.test.tsx`:
    - (a) Column override `format_value($row.bytes, 'bytes')` renders the adaptive value.
    - (b) Column override `format_value($missing, 'bytes')` renders the literal call source AND the column-header icon appears (verify via the rendered icon, not internal reporter call counts).
    - (c) Dedup: same unresolved arg in many rows → one icon, one tooltip entry.
    - (d) Reset on `overrides` change: change the format to a valid one → icon disappears.
    - (e) Naked unresolved `$cell.selected.col` in a column override `format` → preserved source + icon (pins §6 #2 for the table path; `expandCellSelectionMacros` would previously have collapsed to `''`).
    - (f) `$row.col` for a row whose `col` is null (or whose column is missing) → preserved source + icon (pins §6 #4; `expandRowMacros` previously rendered `''`).
    - (g) `$metric.unit` with `metric` a simple-string notebook variable → preserved source + icon (pins §6 #3; the legacy `expandVariableMacros`'s `\\$${name}\\b`-without-lookahead regex would have produced `<metric-value>.unit`).
    - (h) Hidden-timestamp-column rendering: column override `format` = `"Started: $row.start_time"`, `row.start_time` = microsecond BigInt, `allColumns` carries the timestamp type → renders as RFC3339. Preserves the intent of the deleted `describe('OverrideCell with hidden timestamp column')` block (step 13) against the new evaluator.

### Phase 5 — Documentation

16. Update `mkdocs/docs/web-app/notebooks/variables.md`:
    - Add a *Template Functions* subsection under *SQL Macro Substitution* describing the v1 surface and the `format_value(value, unit)` signature with a few examples.
    - Note the unit vocabulary (point to `lib/units.ts` aliases).
    - Note SQL queries do **not** support function calls in v1.

### Phase 6 — Checks

17. From `analytics-web-app/`: `yarn lint`, `yarn type-check`, `yarn test`.

## Files to Modify

| File | Change |
|---|---|
| `analytics-web-app/src/lib/format-value.ts` *(new)* | Shared `formatValueWithUnit` |
| `analytics-web-app/src/lib/__tests__/format-value.test.ts` *(new)* | Unit tests for the shared formatter |
| `analytics-web-app/src/lib/template-functions.ts` *(new)* | `TEMPLATE_FUNCTIONS` registry + `format_value` impl |
| `analytics-web-app/src/components/XYChart.tsx` | Delete local `formatValue` / `formatStatValue`; import from shared module |
| `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` | Add `resolveMacroRaw` and `evaluateTemplate` (single-pass walker) |
| `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts` | Tests for `evaluateTemplate` and `format_value` |
| `analytics-web-app/src/components/map/EventDetailPanel.tsx` | Swap `substituteMacros` → `evaluateTemplate`; render `<TemplateWarningBanner>` above the body |
| `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx` | Swap `substituteMacros` → `evaluateTemplate`; render `<TemplateWarningBanner>` above the body |
| `analytics-web-app/src/components/TemplateWarningBanner.tsx` *(new)* | Amber-bordered warning list used by Map detail panel and Markdown cell |
| `analytics-web-app/src/lib/screen-renderers/warning-reporter.tsx` *(new)* | `WarningReporterContext`, `useColumnWarnings` hook, `<ColumnHeaderWarningIcon>` used by table parents |
| `analytics-web-app/src/lib/screen-renderers/table-utils.tsx` | Refactor `OverrideCell` to call `evaluateTemplate` and post warnings via `WarningReporterContext`; add required `columnName` prop and pass it at the call site; add optional `trailingIcon` prop to `SortHeader` (rendered outside the truncate span); delete `expandVariableMacros` / `expandCellSelectionMacros` / `expandRowMacros` and their now-dead helpers `formatValueForUrl`, `DOT_NOTATION_REGEX`, `BRACKET_NOTATION_REGEX` |
| `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` | Use `useColumnWarnings(options?.overrides)`; provide `WarningReporterContext`; pass `trailingIcon` on each `SortHeader` |
| `analytics-web-app/src/lib/screen-renderers/cells/TransposedTableCell.tsx` | Use `useColumnWarnings(options?.overrides)`; provide `WarningReporterContext`; render `<ColumnHeaderWarningIcon>` inline next to the row label in each `<td>` |
| `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx` | Same wiring as `TableCell` — hook, provider, `trailingIcon` |
| `analytics-web-app/src/lib/screen-renderers/__tests__/table-utils.test.tsx` | Delete legacy `expand*Macros` tests; add `format_value($row.col, 'unit')` test, unresolved-arg test, dedup test, reset-on-overrides test, and the §6 #2/#3/#4 regression pins |
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

### Table warning surface: column-header icon vs. table-banner aggregation

**Chosen: column-header icon driven by a context-based side channel.** An earlier draft of this plan aggregated table warnings into a single `<TemplateWarningBanner>` above the table, sourced by a `useTableWarnings(data, overrides, ctx)` hook that ran a row-capped dry-run pass. Making that hook efficient required memoizing `getAvailableVariables` / `getAvailableCellResults` / `getAvailableCellSelections` in `NotebookRenderer`, wrapping `streamQuery.getTable()` in a `useMemo` in `TableRenderer`, exporting module-level `EMPTY_*` constants, and a `TABLE_WARNINGS_ROW_CAP`. All of that disappears with the icon-based approach: warnings are a render byproduct of `OverrideCell`, not a separate pass; a `useCallback`-stable reporter with reducer-style dedup means the parent re-renders only for *new* warnings; coverage is naturally bounded by what `TableBody` renders. The icon also points authors at the column that's failing, which is more actionable than a banner saying "13 warnings across the table." Trade-off: failures in offscreen rows of paginated tables only surface once those rows are visited — acceptable because schema/typo failures are systematic (whole-column), and once flagged the icon stays sticky across pagination.

### Acknowledgement — warning surface is a deliberate add-on

The core feature here is small: a shared `formatValueWithUnit`, a single-pass walker that resolves macros to raw values for function arguments, and a one-entry registry. The warning surface — `{ text, warnings }` return shape, `<TemplateWarningBanner>` for single-template surfaces, `WarningReporterContext` + `<ColumnHeaderWarningIcon>` for the table — exists because the existing macro engine handles failure poorly (silent collapse for unresolved cell selections, no diagnostic for unresolved variables) and silent failures are an observed user pain point. Compared with an earlier draft that reached for parent-component memoization and a row-capped dry-run pass, this version piggybacks on the render `OverrideCell` already does and uses dedup to absorb redundant per-cell reports — so no memoization or cap is needed. If the SQL macro path ever needs a similar diagnostic surface, the same context primitive can be reused.

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
- `lib/screen-renderers/__tests__/table-utils.test.tsx`: column override with `format_value($row.col, 'bytes')` renders adaptive text; unresolved-arg override preserves source and surfaces `<ColumnHeaderWarningIcon>` via the provider; reset-on-overrides clears the icon; dedup across rows produces a single tooltip entry; naked unresolved `$cell.selected.col` preserves source (pins §6 #2); null `$row.col` preserves source (pins §6 #4); `$metric.unit` with simple-string `metric` preserves source (pins §6 #3 — the boundary-regex behavior change vs. the legacy `expandVariableMacros`).

### Manual tests

1. Notebook with Map cell. Query: `SELECT NOW() as time, 0 as x, 0 as y, 0 as z, 3678630912 as bytes_used`. Detail template: `**Memory:** format_value($bytes_used, 'bytes')`. Verify panel renders `3.4 GB` (size units other than `bytes` render with 1 decimal, matching the chart formatter).
2. Notebook with a variable cell `metric` whose query returns `(name, unit)` rows (e.g. `SELECT 'memory' AS name, 'bytes' AS unit UNION ALL SELECT 'latency', 'seconds'`) — a multi-column variable. Add a Map cell whose query exposes both a value column and the selected metric, e.g. `SELECT NOW() AS time, 0 AS x, 0 AS y, 0 AS z, 3678630912 AS metric_value`. Detail template: `**Value:** format_value($metric_value, $metric.unit)`. Switch the `metric` combobox between `memory` and `latency` and verify the rendered output adapts (`3.4 GB` vs. an adaptive-time format on the same numeric value).
3. Table cell with `bytes_used` column. Column override: `format_value($row.bytes_used, 'bytes')`. Verify each row formats adaptively. Then change the override to `format_value($row.nonexistent, 'bytes')` and verify (a) cells render the literal source text and (b) an amber warning icon appears next to the column header with the unresolved-arg detail in the tooltip.

## Out of Scope (v1, per issue)

- Arithmetic in templates (`$a + $b`)
- User-defined functions
- Conditional expressions
- Function calls inside SQL templates
- Nested function calls
- Function-call argument validation at *edit time*: `validateMacros` (`notebook-utils.ts:436`) and `validateFormatMacros` (`table-utils.tsx:141`) are intentionally **not** updated. Inner `$macro` arguments still validate because the validators scan `$…` patterns anywhere in the text. v1 doesn't check function arity, function-name existence, or unit-name validity at edit time. At *render time*, unresolved-arg calls render as their original source text and surface a warning via the diagnostic surface described in §6a (banner for Map/Markdown, column-header icon for tables); naked unresolved macros (e.g. `$missing`, `$cell.selected.col` with no selection, null `$row.col`) likewise render as source and emit a warning, per the macro-branch rule in *Phase 2 step 5*; unknown function names still pass through unchanged with no warning (matching the "pass through unknown" rule). Adding arity/unit checks to the editor validators is a clean future increment.
- Edit-panel integration of the column-header warnings — surfacing the same `columnWarnings` state inside the column override edit dialog is a clean follow-up; v1 ships with hover-tooltip on the header icon.

## Open Questions

None — ship Phases 1-6 as one PR (per follow-up decision: bundled delivery, not split).
