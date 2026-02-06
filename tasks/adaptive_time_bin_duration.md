# Adaptive time_bin_duration Notebook Variable (was time_bin_interval)

## Status: IMPLEMENTED (pending security review)

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

| File | Change | Status |
|------|--------|--------|
| `notebook-expression-eval.ts` | New file: `snapInterval()` (15 snap levels), `evaluateVariableExpression()` via `new Function()` | Done |
| `notebook-types.ts` | `VariableCellConfig.variableType`: `'number'` → `'expression'`, added `expression?: string` field, added `expressionResult?: string` to `CellState` | Done |
| `cell-registry.ts` | Updated `CellRendererProps.variableType` union to match | Done |
| `VariableCell.tsx` | Removed `number` type, added `expression` type: read-only title bar display, JS expression editor with MDN link and binding hints, `execute()` calls `evaluateVariableExpression()`, `onExecutionComplete()` sets computed value | Done |
| `notebook-expression-eval.test.ts` | New file: 19 tests for `snapInterval` and `evaluateVariableExpression` | Done |
| `VariableCell.test.tsx` | Replaced `number` type tests with `expression` type tests | Done |
| `useCellExecution.test.ts` | Removed `number` variable test (text test remains) | Done |
| `NotebookRenderer.test.tsx` | Updated `createVariableCell` helper type union | Done |
| `cell-registry-mock.ts` | Updated variable description string | Done |

## Verification

- Type-check: clean
- Lint: clean
- Tests: 583/583 passing

## Security Assessment

### Trust Model

Notebooks are **shared between users on the same team**. This means one user's expression code runs in every other team member's browser when they open the notebook. Unlike SQL cells — which execute on the backend constrained by database permissions — expression variables execute arbitrary JavaScript in the viewer's browser session with full access to the page context.

**This is a stored XSS vector.** A malicious or compromised team member could craft an expression that:
- Steals session tokens via `document.cookie` or `localStorage`
- Makes authenticated API requests on behalf of the viewer via `fetch()`
- Reads sensitive data from the DOM (other notebooks, user info)
- Redirects the viewer to a phishing page
- Installs a keylogger on the page

SQL cells do NOT have this risk because they run server-side within database permission boundaries. Expression variables are a **new and broader trust boundary**.

### Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| **Stored XSS via expressions** | **High** | Expression JS runs in the viewer's browser with full page context. A malicious author can exfiltrate credentials, make API calls as the viewer, or manipulate the DOM. Must be sandboxed. |
| **Session hijacking** | **High** | Expressions can read `document.cookie`, `localStorage`, and call `fetch()` to exfiltrate auth tokens to an external server. |
| **Privilege escalation** | **Medium** | If team members have different permission levels, a lower-privileged author could craft expressions that trigger actions using a higher-privileged viewer's session. |
| **Cross-notebook injection** | Low | Variable values from upstream cells are passed as function parameters, not string-interpolated into the expression source. This prevents upstream values from breaking out of the expression. |
| **CSP compatibility** | Medium | `new Function()` requires `'unsafe-eval'` in the Content-Security-Policy. Verify the app's CSP allows this, or add it. Document this requirement. |
| **Denial of service** | Low | An expression like `while(true){}` blocks the viewer's UI thread. Low severity since it only affects the tab. |

### Recommended Mitigations

Choose one of the following sandboxing approaches (ordered by strength):

#### Option A: Sandboxed iframe (Recommended)

Run expressions inside a sandboxed `<iframe>` with a `null` origin:
```html
<iframe sandbox="allow-scripts" srcdoc="..."></iframe>
```
- The expression runs in an isolated origin with **no access** to the parent page's cookies, localStorage, DOM, or fetch credentials
- Communication happens via `postMessage()` — the parent sends the variable bindings, the iframe returns the computed string
- Slight implementation complexity but strong isolation
- `new Function()` still works inside the iframe (the `allow-scripts` permission enables it)

#### Option B: Allowlist-based evaluation

Instead of `new Function()`, parse the expression into an AST and evaluate only allowed operations:
- Arithmetic operators, `Date` construction, `Math` methods, `snap_interval()`
- Block property access on `window`, `document`, `fetch`, `localStorage`, etc.
- Use a lightweight expression parser (e.g., `jsep` + custom evaluator)
- More restrictive — users can only use pre-approved functions and operators
- Simpler security model but limits future expressiveness

#### Option C: Accept the risk with auditing

If the team fully trusts all members and accepts the XSS risk:
- Log which user last modified each expression variable (audit trail)
- Show a visual indicator that a notebook contains expression cells
- Display expression source to the viewer before execution (consent prompt on first open)
- This does NOT eliminate the risk — it only makes it traceable

### Additional Recommendations

1. **Validate CSP early**: Check whether the analytics web app currently sets a Content-Security-Policy header. If it does and `'unsafe-eval'` is absent, the feature will silently fail. Add a startup-time check or document the requirement.
2. **Keep strict mode**: The `"use strict"` directive in the `new Function()` body is a good baseline — don't remove it.
3. **No server-side eval**: Expressions must only ever run in the browser. Never send expression source to the backend for evaluation.
4. **Sanitize saved expressions**: When persisting notebook configs, expression strings are stored as data (JSON string values). Ensure the save/load path does not interpret them as anything other than strings.
5. **Audit trail**: Regardless of sandboxing approach, record which user last edited each expression variable. This makes malicious edits attributable.

### Conclusion

**The `new Function()` approach as currently designed is a stored XSS vulnerability** in a shared-notebook environment. SQL cells don't have this problem because they run server-side with database-level access control. Expression variables run client-side with full browser privileges of the viewing user.

**Recommendation: Implement Option A (sandboxed iframe)** before shipping this feature. It preserves the full JavaScript expressiveness of the current design while isolating expressions from the viewer's session. The implementation cost is moderate — roughly one additional module to manage iframe lifecycle and `postMessage` communication.

## Resolved Questions

1. **Resize triggers re-execution?** No — width is captured at execution time via `window.innerWidth`.
2. **Override computed value?** Expression variables are read-only. Use a text variable for manual control.
3. **Expression field:** Dedicated `expression` field on `VariableCellConfig`, not reusing `sql`.
