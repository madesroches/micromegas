# Notebook Reactive Execution Plan

## Status: Implemented

All phases are complete. The bug fix, auto-run feature, debounced config auto-run, and keystroke fix are all in place.

## Overview

There were two problems to solve:

1. **Bug**: Changing a variable value re-executed the entire notebook due to a spurious time range recomputation.
2. **Feature**: After the bug fix, variable changes do nothing (no re-execution). Users can now opt individual cells into automatic re-execution when their value or config changes, without altering the default manual behavior.

## Problem Statement

Given a notebook like:

```
[source]      datasource variable
[processes]   table, remote query using $source → registers result in WASM
[search]      text variable, default "client"
[search_results]  table, notebook datasource, SQL: WHERE exe ILIKE '%$search%'
```

**Before**: typing in `search` re-executed the *entire* notebook (including the expensive remote `processes` query) due to a bug.

**After**: typing in `search` re-executes from the `search` cell onward (including `search_results` and any cells below), only when the user has enabled "Auto-run from here" on the `search` variable cell.

## Root Cause Analysis

### Bug: Variable changes triggered full notebook re-execution

The root cause was a chain reaction through the time range computation:

1. User types in text input → `VariableCell.handleTextChange` debounces at 300ms → calls `onValueChange`
2. `useNotebookVariables.setVariableValue` calls `setSearchParams` to update URL (`useNotebookVariables.ts:125-138`)
3. React Router's `useSearchParams` creates a **new `searchParams` object** on any URL change — even for unrelated params
4. In `ScreenPage.tsx:101-107`, `rawTimeRange` is a `useMemo` that depends on `searchParams` (the whole object). It recomputes.
5. In `ScreenPage.tsx:131-138`, `apiTimeRange` recomputes. Since the time range is relative (`now-1h` to `now`), `getTimeRangeForApi` calls `new Date()` (`time-range.ts:124`) — producing **different** ISO timestamp strings every time
6. The new `apiTimeRange` with different `begin`/`end` is passed to `useCellExecution`
7. The time-range-change `useEffect` (`useCellExecution.ts:260-269`) sees different strings → calls `executeFromCell(0)` → entire notebook re-executes

**Summary**: variable URL param change → spurious `rawTimeRange` recomputation → `new Date()` produces new timestamps → time range detector fires → full re-execution.

### Key files involved
| File | Role |
|------|------|
| `ScreenPage.tsx` | `rawTimeRange` useMemo previously depended on whole `searchParams` |
| `time-range.ts` | `parseTimeRange` calls `new Date()` on every invocation |
| `useCellExecution.ts` | Time range change detection triggers `executeFromCell(0)` |
| `useNotebookVariables.ts` | `setVariableValue` calls `setSearchParams` |
| `NotebookRenderer.tsx` | Wires variable changes, auto-run logic, config change detection |
| `VariableCell.tsx` | 300ms debounce on text input, `pendingRef` keystroke guard |
| `CellContainer.tsx` | Cell context menu (Radix DropdownMenu) and header |
| `notebook-types.ts` | Cell config types including `autoRunFromHere` |

## Design

### Part 1: Fix the spurious re-execution bug (done)

**Fix in `ScreenPage.tsx`**: Extracted the time-related URL params before the memo instead of depending on the whole `searchParams` object.

```typescript
// Before (broken): depends on entire searchParams object
const rawTimeRange = useMemo(
  () => ({
    from: searchParams.get('from') ?? savedTimeFrom ?? currentTimeFrom!,
    to: searchParams.get('to') ?? savedTimeTo ?? currentTimeTo!,
  }),
  [searchParams, savedTimeFrom, savedTimeTo, currentTimeFrom, currentTimeTo]
)

// After (fixed): depends only on the actual time param values
const urlFrom = searchParams.get('from')
const urlTo = searchParams.get('to')
const rawTimeRange = useMemo(
  () => ({
    from: urlFrom ?? savedTimeFrom ?? currentTimeFrom!,
    to: urlTo ?? savedTimeTo ?? currentTimeTo!,
  }),
  [urlFrom, urlTo, savedTimeFrom, savedTimeTo, currentTimeFrom, currentTimeTo]
)
```

This breaks the chain: variable param changes no longer trigger `rawTimeRange` recomputation, so `apiTimeRange` stays stable, and the time range change detector in `useCellExecution` doesn't fire.

### Part 2: Per-cell "Auto-run from here" flag (done)

With the bug fix in place, variable changes correctly do nothing. The opt-in reactive behavior is now implemented.

`autoRunFromHere` boolean on the cell config enables two auto-run trigger paths:
- **Variable value changes**: changing a variable's value triggers `executeFromCell(cellIndex)` directly
- **Config changes**: editing SQL or other execution-relevant config triggers a debounced (300ms) `executeFromCell(cellIndex)` via `scheduleAutoRun`

**Semantics**: `autoRunFromHere` is a property of the cell being modified. When the user changes a variable that has this flag, the notebook executes from that cell through all cells below, stopping on error. No searching for downstream cells — the flag means "when I change, run from here."

**Why this approach:**
- **Simple mental model**: "enable auto-run on a variable and it re-runs everything below when changed"
- **Reuses existing execution machinery**: just calls `executeFromCell`
- **Granular control**: each variable cell decides independently
- **Cheap by default**: only cells the user explicitly marks will auto-run
- **Works with debounce**: text variables already debounce at 300ms, so auto-run won't fire on every keystroke

#### Config change

```typescript
// In notebook-types.ts
export interface CellConfigBase {
  name: string
  type: CellType
  layout: { height: number; collapsed?: boolean }
  autoRunFromHere?: boolean  // NEW: auto-execute from this cell when its value changes
}
```

#### Execution triggers

There are two auto-run trigger paths, each with different semantics:

**1. Variable value changes** — direct execution in `onValueChange` callback in `NotebookRenderer.renderCell`. Has direct access to `executeFromCell` (no ref indirection needed) and only fires for user-initiated changes. A re-entrance guard (`autoRunningRef`) prevents recursive auto-run when execution itself sets variable values (e.g., auto-selecting the first combobox option).

```typescript
onValueChange: cell.type === 'variable' ? (value: VariableValue) => {
  setVariableValue(cell.name, value)

  // Auto-run: if this cell has autoRunFromHere, execute from here onward.
  if (autoRunningRef.current || !cell.autoRunFromHere) return
  autoRunningRef.current = true
  executeFromCell(index).finally(() => {
    autoRunningRef.current = false
  })
} : undefined,
```

**2. Config changes** — debounced execution via `scheduleAutoRun` in the `updateCell` callback. When an execution-relevant config key changes (e.g., SQL), a 300ms debounced timer fires `executeFromCellRef.current(cellIndex)`. Uses a ref to avoid stale closures in setTimeout. A blocklist (`nonExecKeys`) excludes presentation-only keys (`layout`, `name`, `autoRunFromHere`, `options`) from triggering execution — `options` covers page size, sort, and hidden columns which are view-level settings.

#### UI: toggle in the cell context menu

The cell's `⋮` dropdown menu (powered by `@radix-ui/react-dropdown-menu` with Portal to escape overflow-hidden ancestors) includes an "Auto-run from here" toggle:

```
┌──────────────────────┐
│ ▶ Run from here      │
│ ⚡ Auto-run from here │  ← toggle
│──────────────────────│
│ 🗑 Delete cell        │
└──────────────────────┘
```

When `autoRunFromHere` is enabled, a Zap (⚡) icon appears in the cell header so the user can see at a glance which cells are reactive. The menu item text changes to "Disable auto-run" when active.

#### Debounce considerations

The existing 300ms debounce in `VariableCell.handleTextChange` provides adequate protection for variable value changes. Config changes (SQL editing) use a separate 300ms debounce via `scheduleAutoRun`. No additional debounce is needed because:
- Text variables: already debounced at 300ms before `onValueChange` fires
- Combobox variables: discrete selection, no debounce needed
- Expression variables: set once after execution, no rapid changes
- Datasource variables: discrete selection
- SQL editing: debounced at 300ms via `scheduleAutoRun`

#### Keystroke preservation fix

When auto-run triggers re-execution, the execution cascade can call `setVariableValue` on downstream variables, which updates the `value` prop on `VariableCell`. Without protection, the `useEffect` that syncs `localValue` from `value` would reset the text input mid-typing, losing the last keystroke. A `pendingRef` flag in `useVariableInput` guards against this: while a debounce is pending (`pendingRef.current === true`), the effect skips resetting `localValue`.

### Alternative approaches considered

#### A. Dependency graph with selective re-execution

Build a DAG of cell dependencies and only re-execute cells that depend on the changed variable.

**Rejected**: Over-engineered for current needs. SQL dependency analysis is fragile. The "Run from here" semantic is simpler and matches existing UI. Can be added later as an optimization.

#### B. Global reactivity toggle (Marimo-style)

A notebook-level "auto-run" / "lazy" toggle for all cells.

**Rejected**: Too coarse. The user's example has both expensive remote queries (should NOT auto-run) and cheap WASM queries (should auto-run). Per-cell granularity is strictly more flexible.

#### C. Callback in useNotebookVariables

Add an `onVariableChange` callback parameter to `useNotebookVariables` that fires after `setVariableValue`.

**Rejected during implementation**: This required ref-based indirection to access `executeFromCell`, introducing timing bugs (ref null on first render, permanent lock-up on error). Placing the auto-run logic directly in `onValueChange` in `renderCell` is simpler and avoids these issues — `executeFromCell` is directly in scope.

#### D. Searching downstream for autoRunFromHere cells

When a variable changes, scan all cells below it to find the first one with `autoRunFromHere` and execute from there.

**Rejected**: Over-complicated. The simpler model is: `autoRunFromHere` is a property of the cell being modified. The user enables it on the variable they want to be reactive.

## Implementation Steps (all complete)

### Phase 1: Fix the bug

1. **Stabilize `rawTimeRange` memo** (`ScreenPage.tsx`) — done
   - Extracted `searchParams.get('from')` and `searchParams.get('to')` into `urlFrom`/`urlTo` local variables
   - Used those as `useMemo` dependencies instead of the whole `searchParams` object

### Phase 2: Config and execution plumbing

2. **Add `autoRunFromHere` to cell config** (`notebook-types.ts`) — done
3. **Add auto-run trigger in `NotebookRenderer`** — done
   - `autoRunningRef` re-entrance guard for variable value changes
   - `scheduleAutoRun` debounced execution for config changes
   - `nonExecKeys` blocklist to exclude presentation-only keys from triggering execution
4. **Fix keystroke loss during auto-run cascade** (`VariableCell.tsx`) — done
   - Added `pendingRef` to guard `localValue` reset while debounce is pending
5. **Default text variables to empty string** (`useNotebookVariables.ts`) — done
   - Text variables with no default value now default to `''` so `$variable` macros substitute correctly

### Phase 3: UI

6. **Replace hand-rolled dropdown with Radix DropdownMenu** (`CellContainer.tsx`) — done
7. **Add "Auto-run from here" toggle to cell context menu** (`CellContainer.tsx`) — done
8. **Add Zap indicator to cell header** (`CellContainer.tsx`) — done
9. **Wire toggle through `NotebookRenderer`** — done

## Files Modified

| File | Changes |
|------|---------|
| `ScreenPage.tsx` | Extract `urlFrom`/`urlTo` to fix `rawTimeRange` useMemo dependencies |
| `notebook-types.ts` | Add `autoRunFromHere?: boolean` to `CellConfigBase` |
| `NotebookRenderer.tsx` | Auto-run logic in `onValueChange`, `autoRunningRef` guard, debounced `scheduleAutoRun` for config changes, `nonExecKeys` blocklist, wire props to `CellContainer` |
| `CellContainer.tsx` | Replace dropdown with Radix DropdownMenu + Portal, add Zap indicator and auto-run toggle |
| `VariableCell.tsx` | `pendingRef` to prevent keystroke loss during auto-run cascade |
| `useNotebookVariables.ts` | Text variables with no default value default to empty string |
| `ScreenPage.urlState.test.tsx` | Regression tests for time range stability |
| `CellContainer.test.tsx` | Auto-run toggle tests |
| `NotebookRenderer.test.tsx` | Zap icon mock, menu test updates |
| `jest.config.js` | Shared Radix dropdown mock via `moduleNameMapper` |
| `src/__mocks__/@radix-ui/react-dropdown-menu.tsx` | Shared test mock for Radix DropdownMenu |

**Not modified**: `useCellExecution.ts` (reuses existing `executeFromCell` as-is).

## Testing Strategy

### Regression tests for the bug fix (Phase 1)

`describe('rawTimeRange stability')` block in `ScreenPage.urlState.test.tsx` with a `useTimeRangeComputation` hook that mirrors the fixed `rawTimeRange` → `apiTimeRange` chain.

**Tests:**

1. `rawTimeRange` reference stable on variable change
2. `apiTimeRange` strings stable on variable change
3. Positive control — `rawTimeRange` recomputes on `from` change
4. Positive control — `apiTimeRange` changes on `to` change

### Auto-run feature tests (Phase 2-3)

- CellContainer tests: Zap indicator visibility, auto-run toggle menu item text ("Auto-run from here" / "Disable auto-run"), toggle callback
- NotebookRenderer tests: "Auto-run from here" menu item presence in cell context menu

### Edge cases covered

- Changing a variable without `autoRunFromHere` → nothing auto-executes
- Auto-run while previous auto-run is still executing → `autoRunningRef` guard prevents re-entrance
- Saving notebook persists `autoRunFromHere` flag (standard config persistence)
- Loading notebook with `autoRunFromHere` flags does NOT cause double execution on initial load (auto-run only fires from `onValueChange`)
- View-only settings (page size, sort, hidden columns) do NOT trigger auto-run (`options` in `nonExecKeys`)
- Text input keystrokes are preserved during auto-run cascade (`pendingRef` in `useVariableInput`)
