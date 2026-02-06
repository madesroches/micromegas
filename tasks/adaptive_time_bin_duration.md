# Adaptive time_bin_duration Notebook Variable (was time_bin_interval)

## Status: PLANNED

## Issue Reference
- GitHub Issue: [#778](https://github.com/madesroches/micromegas/issues/778)

## Overview

Add a built-in notebook variable `$time_bin_duration` that automatically computes an optimal time bin size based on the query time range and the browser window width. This makes time-series aggregation queries adaptive to both the viewed time span and available screen resolution.

**Example usage:**
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
- Three types: combobox (SQL-driven dropdown), text, number
- The `number` type is redundant — text variables already cover this use case
- Default values are strings or JSON objects
- No support for expression-based or computed defaults

### Window Width Tracking (`XYChart.tsx`)
- XYChart already has `ResizeObserver` + window resize event handling
- Reports width via `onWidthChange()` callback
- Enforces minimum 400px, subtracts 32px padding

## Implementation Plan

### Step 1: Add `$window_width_px` to CellExecutionContext

**File:** `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`

Add `windowWidthPx` to `CellExecutionContext`:
```typescript
export interface CellExecutionContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>
  windowWidthPx: number
}
```

### Step 2: Track container width in NotebookRenderer

**File:** `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`

- Add a `useContainerWidth()` hook (or inline logic) that uses `ResizeObserver` on the notebook container to track available width
- Pass `windowWidthPx` down to `useCellExecution`

### Step 3: Pass windowWidthPx through execution

**File:** `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`

- Accept `windowWidthPx` as a parameter
- Include it in the `CellExecutionContext` passed to each cell's `execute()` method
- Default to `window.innerWidth` if no container measurement is available

### Step 4: Create expression evaluation utility

**File:** `analytics-web-app/src/lib/screen-renderers/notebook-expression-eval.ts` (new)

Create a utility module for evaluating variable expressions:

```typescript
export function evaluateVariableExpression(
  expression: string,
  context: {
    begin: string
    end: string
    windowWidthPx: number
    variables: Record<string, VariableValue>
  }
): string
```

**Implementation details:**

1. **Standard JS library access**: Expressions use plain JavaScript. The standard library (`Date`, `Math`, `JSON`, etc.) is available. No custom wrappers — users write `new Date($end)` not `datetime($end)`.

2. **`snap_interval()` helper**: The only custom function provided. Snaps a millisecond duration to a human-friendly SQL interval string:
   ```typescript
   function snapInterval(ms: number): string
   ```
   Snap levels: `100ms`, `500ms`, `1s`, `5s`, `15s`, `30s`, `1m`, `5m`, `15m`, `30m`, `1h`, `6h`, `1d`, `7d`, `30d`

3. **Evaluation via `new Function()`**: No macro substitution — `$begin`, `$end`, `$window_width_px`, and upstream variables are passed as JS variable bindings:
   ```typescript
   const paramNames = ['$begin', '$end', '$window_width_px', 'snap_interval',
     ...Object.keys(variables).map(name => `$${name}`)]
   const paramValues = [begin, end, windowWidthPx, snapInterval,
     ...Object.values(variables)]
   const fn = new Function(...paramNames,
     `"use strict"; return (${expression})`)
   return fn(...paramValues)
   ```
   `$begin` and `$end` are strings (ISO 8601), `$window_width_px` is a number, upstream variables are their `VariableValue`. Standard JS globals (`Date`, `Math`, etc.) remain accessible. This is the same trust boundary as the SQL queries the notebook author writes.

4. **Error handling**: Catch evaluation errors and return a sensible fallback (e.g., `'1m'`), logging the error for debugging.

### Step 5: Remove `number` variable type

**File:** `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx`

Remove the `number` variable type — it's redundant with `text`. Text variables already accept any string input, and the value is substituted as-is into SQL. A dedicated numeric type adds UI complexity with no benefit.

- Remove `'number'` from the `variableType` union type
- Remove the number-specific input rendering in the title bar renderer
- Remove number-specific handling in `useVariableInput`
- Any existing notebook configs with `variableType: 'number'` should be treated as `'text'` (migration fallback)

### Step 6: Add expression support to variable cells

**File:** `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx`

Add a new variable type: `expression` (replacing `number` in the type union).

- The `VariableCellConfig` type option becomes: `'combobox' | 'text' | 'expression'`
- Expression variables don't execute SQL — they evaluate a JS expression
- No macro substitution (`substituteMacros` is not called) — `$begin`, `$end`, `$window_width_px`, and upstream variables are real JS bindings passed to `new Function()`
- The `execute()` method for expression variables:
  1. Takes the cell's `sql` field (repurposed as the expression text)
  2. Calls `evaluateVariableExpression()` with the current context
  3. Returns the result as the variable value
- The editor shows a code input for the expression with documentation about available functions
- The title bar renderer shows the computed value (read-only display)

**Config shape:**
```typescript
{
  type: 'variable',
  name: 'time_bin_duration',
  variableType: 'expression',
  sql: "snap_interval((new Date($end) - new Date($begin)) / $window_width_px)",
  defaultValue: '1m'  // fallback if expression fails
}
```

### Step 7: Add `$window_width_px` to macro system

**File:** `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`

- In `substituteMacros()`: Add `$window_width_px` substitution alongside `$begin`/`$end`
  - This requires passing `windowWidthPx` to the function (extend its signature or pass via context object)
- In `validateMacros()`: Add `$window_width_px` to the skip list alongside `$begin`, `$end`, `$order_by`
- Update callers of `substituteMacros` to pass the new context

### Step 8: Update AvailableVariablesPanel

**File:** `analytics-web-app/src/components/AvailableVariablesPanel.tsx`

Add `$window_width_px` to the "Time range" section of built-in variables with description: "Browser viewport width in pixels".

### Step 9: Handle re-execution on window resize

**Design decision:** Window resize should NOT trigger automatic re-execution. Instead:
- The `$window_width_px` value is captured at execution time
- When the user clicks "Run All" or triggers execution, the current width is used
- This avoids disruptive re-execution during resize and is consistent with how `$begin`/`$end` work (changing the time range doesn't auto-execute)

### Step 10: Tests

**File:** `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-expression-eval.test.ts` (new)

Test cases:
- Standard JS works: `new Date($end) - new Date($begin)` produces correct ms
- `Math.round()`, `Math.max()` etc. accessible in expressions
- `snap_interval()` snaps to correct human-friendly intervals:
  - 750ms -> '500ms', 1200ms -> '1s', 8000ms -> '5s', 180000ms -> '5m', etc.
- Full expression evaluation end-to-end
- Error handling: malformed expression returns fallback
- Expression variables integrate into substitution correctly

**File:** `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts` (extend)

- `$window_width_px` substitution works
- `$window_width_px` passes validation (not flagged as unknown)

## File Change Summary

| File | Change |
|------|--------|
| `cell-registry.ts` | Add `windowWidthPx` to `CellExecutionContext` |
| `notebook-expression-eval.ts` | New file: expression evaluation, `datetime()`, `snap_interval()` |
| `VariableCell.tsx` | Remove `number` type, add `expression` type with evaluate-on-execute |
| `notebook-utils.ts` | Add `$window_width_px` to `substituteMacros` and `validateMacros` |
| `useCellExecution.ts` | Pass `windowWidthPx` into execution context |
| `NotebookRenderer.tsx` | Track container width, pass to execution |
| `AvailableVariablesPanel.tsx` | Show `$window_width_px` in built-in variables |
| `notebook-expression-eval.test.ts` | New file: tests for expression evaluation |
| `notebook-utils.test.ts` | Extend: tests for `$window_width_px` |

## Execution Order

1. Step 5 (remove `number` variable type) — do first to simplify VariableCell before adding expression
2. Step 4 (expression eval utility + Step 10 tests) — standalone
3. Step 1 (context type change) — type-level only
4. Steps 2 & 3 (width tracking + context wiring) — depend on Step 1
5. Step 7 (macro substitution) — depends on Step 3
6. Step 6 (expression variable type) — depends on Steps 1-5 & 7
7. Step 8 (panel update) — depends on Step 7
8. Step 9 is a design constraint, not a code step

## Open Questions

1. **Resize triggers re-execution?** Plan says no — consistent with `$begin`/`$end` behavior. Width is captured at execution time.
2. **Override computed value?** Expression variables show the computed value read-only. Users who want manual control can use a regular text variable named differently.
3. **Expression field reuse:** The plan reuses the `sql` field for expression text. An alternative is adding a dedicated `expression` field to `VariableCellConfig`. Using `sql` is simpler but less semantic — worth discussing.
