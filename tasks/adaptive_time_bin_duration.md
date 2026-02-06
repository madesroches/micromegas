# Adaptive time_bin_duration Notebook Variable (was time_bin_interval)

## Status: IMPLEMENTED

## Issue Reference
- GitHub Issue: [#778](https://github.com/madesroches/micromegas/issues/778)

## Overview

Add an `expression` variable type that evaluates JavaScript in the browser. This enables computed variables like `time_bin_duration` that automatically derive an optimal time bin size from the query time range and the browser window width.

**Example expression variable:**
```javascript
snap_interval($duration_ms / $innerWidth)
```

**Example SQL usage (in a query cell below):**
```sql
SELECT
  time_bucket('$time_bin_duration', time) AS bucket,
  avg(value) AS avg_value
FROM measures
WHERE time BETWEEN '$begin' AND '$end'
GROUP BY bucket
ORDER BY bucket
```

## Current Architecture

### Macro Substitution (`notebook-utils.ts`)
- `substituteMacros()` handles three macro types: time range (`$begin`/`$end`), dotted variable refs (`$var.col`), and simple variable refs (`$var`)
- `validateMacros()` skips built-in variables: `$begin`, `$end`, `$order_by`
- Variables are processed in descending name-length order to avoid partial matches

### CellExecutionContext (`cell-registry.ts`)
```typescript
export interface CellExecutionContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>
}
```

### Variable Cells (`VariableCell.tsx`)
- Three types: combobox (SQL-driven dropdown), text, and expression (allowlist-based AST evaluator)
- Default values are strings or JSON objects (combobox/text) or computed at execution time (expression)
- Expression variables use `evaluateVariableExpression()` with named bindings, no arbitrary JS execution

## Implementation Plan

### Step 1: Create expression evaluation utility

**File:** `analytics-web-app/src/lib/screen-renderers/notebook-expression-eval.ts` (new)

Create a utility module for evaluating variable expressions:

```typescript
export function evaluateVariableExpression(
  expression: string,
  context: ExpressionContext
): string

export interface ExpressionContext {
  begin: string
  end: string
  durationMs: number
  innerWidth: number
  devicePixelRatio: number
  variables: Record<string, VariableValue>
}
```

**Implementation details:**

1. **Allowlist-based evaluation**: Instead of `new Function()`, expressions are parsed into an AST using a lightweight parser (e.g., [`jsep`](https://github.com/EricSmekworthy/jsep) ~2KB, zero deps) and evaluated by a recursive walker that only permits allowed operations. This prevents arbitrary JS execution in shared notebooks.

2. **Allowed operations**:
   - Binary operators: `+`, `-`, `*`, `/`, `%`
   - Unary operators: `-`, `+`
   - `new Date(value)` — Date construction
   - `Math.*` — all Math static methods and constants (`Math.round`, `Math.max`, `Math.PI`, etc.)
   - `snap_interval(ms)` — custom helper (see below)
   - Variable references: `$begin`, `$end`, `$duration_ms`, `$innerWidth`, `$devicePixelRatio`, upstream `$variables`
   - Numeric and string literals

3. **Blocked operations** (the evaluator throws on these):
   - Property access on anything other than `Math` (no `window`, `document`, `globalThis`)
   - Function calls other than `Date`, `Math.*`, and `snap_interval`
   - Assignment, template literals, arrow functions, `eval`, `Function`

4. **`snap_interval()` helper**: Snaps a millisecond duration to a human-friendly SQL interval string:
   ```typescript
   function snapInterval(ms: number): string
   ```
   Snap levels: `100ms`, `500ms`, `1s`, `5s`, `15s`, `30s`, `1m`, `5m`, `15m`, `30m`, `1h`, `6h`, `1d`, `7d`, `30d`

5. **Built-in bindings**: The evaluator provides these as named values (not global object access):
   - `$begin`, `$end` — ISO 8601 strings from the time range
   - `$duration_ms` — time range duration in milliseconds (`new Date(end) - new Date(begin)`), computed at execution time
   - `$innerWidth` — `window.innerWidth` captured at execution time
   - `$devicePixelRatio` — `window.devicePixelRatio` captured at execution time
   - `snap_interval` — the snap helper function
   - `$<name>` — upstream variable values

6. **Error handling**: Let evaluation errors propagate. No fallback — if the expression is broken, the user should see the error.

### Step 2: Remove `number` variable type

**File:** `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx`

Remove the `number` variable type — it's redundant with `text`. Text variables already accept any string input, and the value is substituted as-is into SQL. A dedicated numeric type adds UI complexity with no benefit.

- Remove `'number'` from the `variableType` union type
- Remove the number-specific input rendering in the title bar renderer
- Remove number-specific handling in `useVariableInput`

### Step 3: Add expression support to variable cells

**File:** `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx`

Add a new variable type: `expression` (replacing `number` in the type union).

- The `VariableCellConfig` type option becomes: `'combobox' | 'text' | 'expression'`
- Expression variables don't execute SQL — they evaluate an expression via the allowlist-based AST evaluator
- No macro substitution (`substituteMacros` is not called) — `$begin`, `$end`, `$duration_ms`, `$innerWidth`, `$devicePixelRatio`, and upstream variables are provided as named bindings to the evaluator
- **Key change to `execute()`:** Currently, text/number variables short-circuit with `return null` (no execution needed). Expression variables must actually run during `execute()`:
  1. Takes the cell's `expression` field (dedicated field, not `sql`)
  2. Calls `evaluateVariableExpression()` with the current context
  3. Sets the variable value to the result (so downstream cells see the computed value)
- This means expression values are recomputed on every notebook execution, reflecting the current time range and window width
- The editor must make clear this is **an expression, not SQL**:
  - Label the input as "Expression"
  - Show a brief inline hint listing the available bindings (`$begin`, `$end`, `$duration_ms`, `$innerWidth`, `$devicePixelRatio`, `snap_interval()`, upstream `$variables`) and allowed operations (`Math.*`, `new Date()`, arithmetic)
- The title bar renderer shows the computed value (read-only display)

**Config shape:**
```typescript
{
  type: 'variable',
  name: 'time_bin_duration',
  variableType: 'expression',
  expression: "snap_interval($duration_ms / $innerWidth)",
}
```

### Step 4: Handle re-execution on window resize

**Design decision:** Window resize should NOT trigger automatic re-execution. Instead:
- `$innerWidth` is set to `window.innerWidth` at execution time
- When the user clicks "Run All" or triggers execution, the current width is used
- This avoids disruptive re-execution during resize and is consistent with how `$begin`/`$end` work (changing the time range doesn't auto-execute)

### Step 5: Tests

**File:** `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-expression-eval.test.ts` (new)

Test cases:
- Arithmetic: `$innerWidth * 2` produces correct result
- `$duration_ms` is correctly computed from time range
- `$devicePixelRatio` is available as a binding
- Date arithmetic: `new Date($end) - new Date($begin)` produces correct ms
- `Math.round()`, `Math.max()` etc. accessible in expressions
- `snap_interval()` snaps to correct human-friendly intervals:
  - 750ms -> '500ms', 1200ms -> '1s', 8000ms -> '5s', 180000ms -> '5m', etc.
- Full expression evaluation end-to-end: `snap_interval($duration_ms / $innerWidth)`
- Error handling: malformed expression throws
- **Security**: property access on `window`, `document`, `globalThis` is rejected
- **Security**: function calls other than `Date`, `Math.*`, `snap_interval` are rejected
- **Security**: assignment expressions are rejected
- Expression variables integrate into execution correctly

## File Change Summary

| File | Change |
|------|--------|
| `notebook-expression-eval.ts` | New file: jsep AST parser + recursive allowlist evaluator, `snapInterval()` (15 snap levels), `ExpressionContext` interface |
| `notebook-types.ts` | `VariableCellConfig.variableType`: `'number'` → `'expression'`, added `expression?: string` field, added `expressionResult?: string` to `CellState` |
| `cell-registry.ts` | Updated `CellRendererProps.variableType` union to match |
| `VariableCell.tsx` | Removed `number` type, added `expression` type: read-only title bar display, expression editor with binding/operator hints, `execute()` passes `ExpressionContext` with `$innerWidth`/`$devicePixelRatio`/`$duration_ms`, `onExecutionComplete()` sets computed value |
| `notebook-expression-eval.test.ts` | New file: 15 snap tests, 5 basic arithmetic, 5 binding, 4 function, 2 end-to-end, 2 error handling, 20 security rejection tests |
| `VariableCell.test.tsx` | Replaced `number` type tests with `expression` type tests |
| `useCellExecution.test.ts` | Removed `number` variable test (text test remains) |
| `NotebookRenderer.test.tsx` | Updated `createVariableCell` helper type union |
| `cell-registry-mock.ts` | Updated variable description string |
| `package.json` | Added `jsep` and `@jsep-plugin/new` dependencies |

## Evaluator Design

### Allowed AST nodes
| Node Type | Allowed | Notes |
|-----------|---------|-------|
| `Literal` | numbers, strings | No regex literals |
| `Identifier` | Only known bindings: `$begin`, `$end`, `$duration_ms`, `$innerWidth`, `$devicePixelRatio`, `snap_interval`, upstream `$variables`, `Math` | Throws on unknown identifiers |
| `BinaryExpression` | `+`, `-`, `*`, `/`, `%` | No bitwise, no logical, no comparison |
| `UnaryExpression` | `-`, `+` | No `!`, `~`, `typeof`, `void`, `delete` |
| `CallExpression` | `snap_interval(...)`, `Math.<method>(...)` | Callee must be an allowed identifier or `Math.*` member |
| `MemberExpression` | `Math.<name>` only | Dot notation only (not computed). Blocks `constructor`, `__proto__`, `prototype` |
| `NewExpression` | `new Date(...)` only | Via `@jsep-plugin/new` |
| Everything else | **Rejected** | `ConditionalExpression`, `ArrayExpression`, `ThisExpression`, `SequenceExpression`, etc. |

## Verification

- Type-check: clean
- Lint: clean
- Tests: 611/611 passing (28 new tests vs. previous 583)

## Security Assessment

### Trust Model

Notebooks are **shared between users on the same team**. One user's expression runs in every other team member's browser when they open the notebook. Unlike SQL cells — which execute on the backend constrained by database permissions — expression variables execute client-side.

### Mitigation: Allowlist-based AST evaluation

**Decision: Option B — allowlist-based evaluation with `$innerWidth` binding.**

The current `new Function()` implementation is replaced with a `jsep` AST parser + recursive evaluator that only permits:
- Arithmetic operators (`+`, `-`, `*`, `/`, `%`)
- `new Date()`, `Math.*`, `snap_interval()`
- Variable references (`$begin`, `$end`, `$duration_ms`, `$innerWidth`, `$devicePixelRatio`, upstream `$variables`)
- Numeric and string literals

The evaluator **rejects** any AST node that doesn't match the allowlist, including:
- Property access on anything other than `Math` (blocks `window`, `document`, `globalThis`, `this`)
- Function calls other than `Date`, `Math.*`, and `snap_interval`
- Assignment, template literals, arrow functions

`window.innerWidth` is **not** exposed as a global. Instead, `$innerWidth` is passed as a plain numeric binding, just like `$begin` and `$end`. This eliminates the need for any `window` object access.

### Residual Risks

| Risk | Severity | Notes |
|------|----------|-------|
| **Allowlist bypass** | Low | If the AST evaluator has a bug that lets an unexpected node type through. Mitigated by defaulting to rejection (throw on unknown node types) and security-focused tests. |
| **Upstream variable injection** | Low | Variable values are data inputs to the evaluator, not code. A variable value of `"fetch('/steal')"` is just a string, not executed. |
| **DoS via deep expressions** | Low | Deeply nested arithmetic could cause stack overflow. Acceptable — only affects the author's own tab, and trivially fixed by adding a depth limit if needed. |
| **CSP** | None | No `'unsafe-eval'` required — `jsep` parses strings without `eval` or `new Function()`. |

### Recommendations

1. **Default-deny in the evaluator**: Throw on any unrecognized AST node type. Never silently skip.
2. **No server-side eval**: Expressions must only ever run in the browser.
3. **Sanitize saved expressions**: Expression strings are stored as JSON string values in the notebook config. Ensure the save/load path does not interpret them as anything other than strings.
4. **Test the rejection cases**: Include tests that verify `document.cookie`, `fetch()`, `window.location`, assignment, and other dangerous patterns are rejected.

## Resolved Questions

1. **Resize triggers re-execution?** No — `$innerWidth` is captured from `window.innerWidth` at execution time.
2. **Override computed value?** Expression variables are read-only. Use a text variable for manual control.
3. **Expression field:** Dedicated `expression` field on `VariableCellConfig`, not reusing `sql`.
4. **Security approach?** Allowlist-based AST evaluation (Option B) with named bindings instead of global object access. No `new Function()`, no `eval`, no `unsafe-eval` CSP requirement.
5. **Environment bindings?** `$innerWidth`, `$devicePixelRatio`, `$duration_ms` — captured at execution time. No direct `window` access.
