# Adaptive time_bin_duration Notebook Variable (was time_bin_interval)

## Status: PLANNED

## Issue Reference
- GitHub Issue: [#778](https://github.com/madesroches/micromegas/issues/778)

## Overview

Add an `expression` variable type that evaluates JavaScript in the browser. This enables computed variables like `time_bin_duration` that automatically derive an optimal time bin size from the query time range and the browser window width.

**Example expression variable:**
```javascript
snap_interval((new Date($end) - new Date($begin)) / window.innerWidth)
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
- Two types: combobox (SQL-driven dropdown) and text
- Default values are strings or JSON objects
- No support for expression-based or computed defaults

## Implementation Plan

### Step 1: Create expression evaluation utility

**File:** `analytics-web-app/src/lib/screen-renderers/notebook-expression-eval.ts` (new)

Create a utility module for evaluating variable expressions:

```typescript
export function evaluateVariableExpression(
  expression: string,
  context: {
    begin: string
    end: string
    variables: Record<string, VariableValue>
  }
): string
```

**Implementation details:**

1. **Standard JS library access**: Expressions use plain JavaScript. The standard library (`Date`, `Math`, `JSON`, etc.) is available. No custom wrappers — users write `new Date($end)` not `datetime($end)`. `window.innerWidth` is accessible directly as a JS global.

2. **`snap_interval()` helper**: The only custom function provided. Snaps a millisecond duration to a human-friendly SQL interval string:
   ```typescript
   function snapInterval(ms: number): string
   ```
   Snap levels: `100ms`, `500ms`, `1s`, `5s`, `15s`, `30s`, `1m`, `5m`, `15m`, `30m`, `1h`, `6h`, `1d`, `7d`, `30d`

3. **Evaluation via `new Function()`**: No macro substitution — `$begin`, `$end`, and upstream variables are passed as JS variable bindings. `window.innerWidth` is available as a standard JS global (no need to pass it explicitly):
   ```typescript
   const paramNames = ['$begin', '$end', 'snap_interval',
     ...Object.keys(variables).map(name => `$${name}`)]
   const paramValues = [begin, end, snapInterval,
     ...Object.values(variables)]
   const fn = new Function(...paramNames,
     `"use strict"; return (${expression})`)
   return fn(...paramValues)
   ```
   `$begin` and `$end` are strings (ISO 8601), upstream variables are their `VariableValue`. Standard JS globals (`Date`, `Math`, `window`, etc.) remain accessible. This is the same trust boundary as the SQL queries the notebook author writes.

4. **Error handling**: Let evaluation errors propagate. No fallback — if the expression is broken, the user should see the error.

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
- Expression variables don't execute SQL — they evaluate a JS expression
- No macro substitution (`substituteMacros` is not called) — `$begin`, `$end`, and upstream variables are passed as JS bindings via `new Function()`. `window.innerWidth` is available as a standard JS global.
- **Key change to `execute()`:** Currently, text/number variables short-circuit with `return null` (no execution needed). Expression variables must actually run during `execute()`:
  1. Takes the cell's `expression` field (dedicated field, not `sql`)
  2. Calls `evaluateVariableExpression()` with the current context
  3. Sets the variable value to the result (so downstream cells see the computed value)
- This means expression values are recomputed on every notebook execution, reflecting the current time range and window width
- The editor must make clear this is **JavaScript, not SQL**, and that it runs in the browser:
  - Label the input as "JavaScript expression" (not just "expression")
  - Include a help link to the MDN JavaScript reference (or similar) so users know what's available
  - Show a brief inline hint listing the available bindings (`$begin`, `$end`, `snap_interval()`, upstream `$variables`, `window.innerWidth`)
- The title bar renderer shows the computed value (read-only display)

**Config shape:**
```typescript
{
  type: 'variable',
  name: 'time_bin_duration',
  variableType: 'expression',
  expression: "snap_interval((new Date($end) - new Date($begin)) / window.innerWidth)",
}
```

### Step 4: Handle re-execution on window resize

**Design decision:** Window resize should NOT trigger automatic re-execution. Instead:
- The `window.innerWidth` value is captured at execution time
- When the user clicks "Run All" or triggers execution, the current width is used
- This avoids disruptive re-execution during resize and is consistent with how `$begin`/`$end` work (changing the time range doesn't auto-execute)

### Step 5: Tests

**File:** `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-expression-eval.test.ts` (new)

Test cases:
- Standard JS works: `new Date($end) - new Date($begin)` produces correct ms
- `Math.round()`, `Math.max()` etc. accessible in expressions
- `snap_interval()` snaps to correct human-friendly intervals:
  - 750ms -> '500ms', 1200ms -> '1s', 8000ms -> '5s', 180000ms -> '5m', etc.
- Full expression evaluation end-to-end
- Error handling: malformed expression throws
- Expression variables integrate into execution correctly

## File Change Summary

| File | Change |
|------|--------|
| `notebook-expression-eval.ts` | New file: expression evaluation, `snap_interval()` |
| `VariableCell.tsx` | Remove `number` type, add `expression` type with evaluate-on-execute |
| `notebook-expression-eval.test.ts` | New file: tests for expression evaluation |

## Execution Order

1. Step 2 (remove `number` variable type) — do first to simplify VariableCell before adding expression
2. Step 1 (expression eval utility + Step 5 tests) — standalone, can parallel with Step 2
3. Step 3 (expression variable type) — depends on Steps 1-2
4. Step 4 is a design constraint, not a code step

## Resolved Questions

1. **Resize triggers re-execution?** No — width is captured at execution time via `window.innerWidth`.
2. **Override computed value?** Expression variables are read-only. Use a text variable for manual control.
3. **Expression field:** Dedicated `expression` field on `VariableCellConfig`, not reusing `sql`.
