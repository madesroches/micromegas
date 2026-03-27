# Halt Notebook Execution on Blocked Selection Plan

## Overview

When a notebook cell references a `$cell.selected.column` macro and no row is selected in the upstream cell, execution should halt — no further downstream cells should execute. Currently, the blocked cell is marked correctly but execution continues to subsequent cells that may not depend on the selection, leading to confusing partial results.

## Current State

In `useCellExecution.ts`, when `executeCell` detects an unresolved selection macro, it marks the cell as `'blocked'` but returns `true`:

```typescript
// useCellExecution.ts:150-163
if (unresolvedCell) {
  completeCellExecution(cell.name, {
    status: 'blocked',
    data: [],
    error: `Select a row in "${unresolvedCell}" to view results`,
  })
  return true // don't block downstream — they may not depend on selections
}
```

The `executeFromCell` loop (line 326-342) only halts when `executeCell` returns `false`. Since blocked-on-selection returns `true`, all downstream cells continue executing regardless.

## Design

Change `return true` to `return false` on line 161 so that the `executeFromCell` loop treats a selection-blocked cell the same as an error — it stops execution and marks all remaining downstream cells as blocked.

This is consistent with how the loop already handles execution failures (line 326-342): when `executeCell` returns `false`, remaining cells are marked as `'blocked'` with the generic "Waiting for cell above to succeed" message.

## Implementation Steps

1. In `useCellExecution.ts` line 161, change `return true` to `return false`
2. Update the comment on the same line to explain the new behavior
3. Add a test case in `__tests__/useCellExecution.test.ts` verifying that downstream cells are blocked when a cell has an unresolved selection macro

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts` — change return value
- `analytics-web-app/src/lib/screen-renderers/__tests__/useCellExecution.test.ts` — add test

## Trade-offs

**Alternative: selective blocking** — only block downstream cells that also reference the same selection. This would require scanning all downstream cells' SQL for the same macro, adding complexity. The simpler halt-all approach is chosen because:
- A notebook with selection dependencies typically has all downstream cells depending on the selected row
- It's easier to reason about: if execution can't proceed, it stops
- It matches the existing error-halts-everything behavior

## Testing Strategy

- Unit test: create a notebook with 3 cells where cell B references `$A.selected.col` and cell C has independent SQL. Verify that when A has no selection, both B and C end up blocked.
- Manual test: open a notebook with selection-dependent cells, confirm no cells below the blocked one execute until a row is selected.
