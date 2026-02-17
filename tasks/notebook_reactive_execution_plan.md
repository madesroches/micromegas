# Notebook Reactive Execution Plan

## Overview

There are two problems to solve:

1. **Bug**: Changing a variable value re-executes the entire notebook due to a spurious time range recomputation. This must be fixed first.
2. **Feature**: After the bug is fixed, variable changes will do nothing (no re-execution). We want to let users opt individual variable cells into automatic re-execution when their value changes, without altering the default manual behavior.

## Problem Statement

Given a notebook like:

```
[source]      datasource variable
[processes]   table, remote query using $source → registers result in WASM
[search]      text variable, default "client"
[search_results]  table, notebook datasource, SQL: WHERE exe ILIKE '%$search%'
```

**Today**: typing in `search` re-executes the *entire* notebook (including the expensive remote `processes` query) due to a bug.

**Goal**: typing in `search` should re-execute from the `search` cell onward (including `search_results` and any cells below), only when the user has enabled "Auto-run from here" on the `search` variable cell.

## Current State

### Bug: Variable changes trigger full notebook re-execution

The root cause is a chain reaction through the time range computation:

1. User types in text input → `VariableCell.handleTextChange` debounces at 300ms → calls `onValueChange`
2. `useNotebookVariables.setVariableValue` calls `setSearchParams` to update URL (`useNotebookVariables.ts:125-138`)
3. React Router's `useSearchParams` creates a **new `searchParams` object** on any URL change — even for unrelated params
4. In `ScreenPage.tsx:101-107`, `rawTimeRange` is a `useMemo` that depends on `searchParams` (the whole object). It recomputes.
5. In `ScreenPage.tsx:131-138`, `apiTimeRange` recomputes. Since the time range is relative (`now-1h` to `now`), `getTimeRangeForApi` calls `new Date()` (`time-range.ts:124`) — producing **different** ISO timestamp strings every time
6. The new `apiTimeRange` with different `begin`/`end` is passed to `useCellExecution`
7. The time-range-change `useEffect` (`useCellExecution.ts:260-269`) sees different strings → calls `executeFromCell(0)` → entire notebook re-executes

**Summary**: variable URL param change → spurious `rawTimeRange` recomputation → `new Date()` produces new timestamps → time range detector fires → full re-execution.

### Key files
| File | Role |
|------|------|
| `ScreenPage.tsx:101-107` | `rawTimeRange` useMemo depends on whole `searchParams` |
| `time-range.ts:123-124` | `parseTimeRange` calls `new Date()` on every invocation |
| `useCellExecution.ts:260-269` | Time range change detection triggers `executeFromCell(0)` |
| `useNotebookVariables.ts:125-138` | `setVariableValue` calls `setSearchParams` |
| `NotebookRenderer.tsx:517` | Wires variable changes to `setVariableValue` |
| `VariableCell.tsx:69-77` | 300ms debounce on text input |
| `CellContainer.tsx` | Cell context menu and header |
| `notebook-types.ts:102-137` | Cell config types |

## Design

### Part 1: Fix the spurious re-execution bug

**Fix in `ScreenPage.tsx`**: Extract the time-related URL params before the memo instead of depending on the whole `searchParams` object.

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

### Part 2: Per-cell "Auto-run from here" flag

After the bug fix, variable changes will correctly do nothing. Now we add the opt-in reactive behavior.

Add an `autoRunFromHere` boolean to the cell config. When enabled on a variable cell, changing that variable's value triggers `executeFromCell(cellIndex)` — executing from the variable cell onward. This is the same action as clicking "Run from here" manually.

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

#### Execution trigger

The auto-run logic lives directly in the `onValueChange` callback in `NotebookRenderer.renderCell`. This is the handler called when a user changes a variable's value. It has direct access to `executeFromCell` (no ref indirection needed) and only fires for user-initiated changes (not execution-triggered `setVariableValue` calls from `onExecutionComplete`).

A re-entrance guard (`autoRunningRef`) prevents recursive auto-run when execution itself sets variable values (e.g., auto-selecting the first combobox option).

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

The existing 300ms debounce in `VariableCell.handleTextChange` provides adequate protection. No additional debounce is needed because:
- Text variables: already debounced at 300ms before `onValueChange` fires
- Combobox variables: discrete selection, no debounce needed
- Expression variables: set once after execution, no rapid changes
- Datasource variables: discrete selection

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

## Implementation Steps

### Phase 1: Fix the bug

1. **Stabilize `rawTimeRange` memo** (`ScreenPage.tsx`)
   - Extract `searchParams.get('from')` and `searchParams.get('to')` into local variables
   - Use those as `useMemo` dependencies instead of the whole `searchParams` object

### Phase 2: Config and execution plumbing

2. **Add `autoRunFromHere` to cell config** (`notebook-types.ts`)
   - Add `autoRunFromHere?: boolean` to `CellConfigBase`

3. **Add auto-run trigger in `NotebookRenderer`** (`NotebookRenderer.tsx`)
   - Add `autoRunningRef` re-entrance guard
   - In the `onValueChange` callback in `renderCell`: if `cell.autoRunFromHere`, call `executeFromCell(index)`

### Phase 3: UI

4. **Replace hand-rolled dropdown with Radix DropdownMenu** (`CellContainer.tsx`)
   - Use `@radix-ui/react-dropdown-menu` with `DropdownMenu.Portal` to escape `overflow-hidden` ancestors (variable cells are auto-collapsed, so the old absolutely-positioned menu was clipped)

5. **Add "Auto-run from here" toggle to cell context menu** (`CellContainer.tsx`)
   - Add `autoRunFromHere?: boolean` and `onToggleAutoRunFromHere?: () => void` props
   - Render toggle item in Radix dropdown menu below "Run from here"
   - Only show for cells that `canRun`

6. **Add auto-run indicator to cell header** (`CellContainer.tsx`)
   - When `autoRunFromHere` is true, show a Zap icon in the header

7. **Wire toggle through `NotebookRenderer`** (`NotebookRenderer.tsx`)
   - Pass `autoRunFromHere` and `onToggleAutoRunFromHere` props to `CellContainer`
   - `onToggleAutoRunFromHere` calls `updateCell(index, { autoRunFromHere: !cell.autoRunFromHere })`

## Files Modified

| File | Changes |
|------|---------|
| `ScreenPage.tsx` | Extract `urlFrom`/`urlTo` to fix `rawTimeRange` useMemo dependencies |
| `notebook-types.ts` | Add `autoRunFromHere?: boolean` to `CellConfigBase` |
| `NotebookRenderer.tsx` | Auto-run logic in `onValueChange`, `autoRunningRef` guard, wire props to `CellContainer` |
| `CellContainer.tsx` | Replace dropdown with Radix DropdownMenu + Portal, add Zap indicator and auto-run toggle |
| `ScreenPage.urlState.test.tsx` | Regression tests for time range stability |
| `CellContainer.test.tsx` | Radix mock, auto-run toggle tests |
| `NotebookRenderer.test.tsx` | Radix mock, Zap icon mock, menu test updates |

**Not modified**: `useNotebookVariables.ts` (auto-run logic lives in `NotebookRenderer` instead), `useCellExecution.ts` (reuses existing `executeFromCell` as-is).

## Testing Strategy

### Regression tests for the bug fix (Phase 1)

Added `describe('rawTimeRange stability')` block in `ScreenPage.urlState.test.tsx` with a `useTimeRangeComputation` hook that mirrors the fixed `rawTimeRange` → `apiTimeRange` chain.

**Tests:**

1. **`rawTimeRange` reference stable on variable change**: Assert same object reference (`toBe`) after adding a variable URL param.
2. **`apiTimeRange` strings stable on variable change**: Assert `begin`/`end` strings identical after variable change.
3. **Positive control — `rawTimeRange` recomputes on `from` change**: Assert different reference when `from` param changes.
4. **Positive control — `apiTimeRange` changes on `to` change**: Assert different `end` string when `to` param changes.

### Auto-run feature tests (Phase 2-3)

- CellContainer tests: Zap indicator visibility, auto-run toggle menu item text ("Auto-run from here" / "Disable auto-run"), toggle callback.
- NotebookRenderer tests: "Auto-run from here" menu item presence in cell context menu.
- Manual testing with processes-notebook: enable auto-run on `search` variable, type in text, verify `search_results` updates while `processes` is NOT re-executed.

### Edge cases

- Changing a variable without `autoRunFromHere` → nothing auto-executes
- Auto-run while previous auto-run is still executing → `autoRunningRef` guard prevents re-entrance
- Saving notebook persists `autoRunFromHere` flag (standard config persistence)
- Loading notebook with `autoRunFromHere` flags does NOT cause double execution on initial load (auto-run only fires from `onValueChange`)
