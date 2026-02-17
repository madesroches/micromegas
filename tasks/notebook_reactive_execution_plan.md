# Notebook Reactive Execution Plan

## Overview

There are two problems to solve:

1. **Bug**: Changing a variable value re-executes the entire notebook due to a spurious time range recomputation. This must be fixed first.
2. **Feature**: After the bug is fixed, variable changes will do nothing (no re-execution). We want to let users opt individual cells into automatic re-execution when upstream variables change, without altering the default manual behavior.

## Problem Statement

Given a notebook like:

```
[source]      datasource variable
[processes]   table, remote query using $source → registers result in WASM
[search]      text variable, default "client"
[search_results]  table, notebook datasource, SQL: WHERE exe ILIKE '%$search%'
```

**Today**: typing in `search` re-executes the *entire* notebook (including the expensive remote `processes` query) due to a bug.

**Goal**: typing in `search` should re-execute only `search_results` (and cells below it), only when the user has opted in via an "Auto-run from here" flag on that cell.

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

### Execution triggers (all in `useCellExecution.ts`)
- Initial load (line 242-248)
- Refresh button via `refreshTrigger` prop (line 251-257)
- Time range change (line 260-269) ← **this is the one that fires spuriously**
- WASM engine ready (line 274-280)
- Manual "Run" / "Run from here" buttons (via `executeCell` / `executeFromCell`)

### Key files
| File | Role |
|------|------|
| `ScreenPage.tsx:101-107` | `rawTimeRange` useMemo depends on whole `searchParams` |
| `time-range.ts:123-124` | `parseTimeRange` calls `new Date()` on every invocation |
| `useCellExecution.ts:260-269` | Time range change detection triggers `executeFromCell(0)` |
| `useNotebookVariables.ts:125-138` | `setVariableValue` calls `setSearchParams` |
| `NotebookRenderer.tsx:517` | Wires variable changes to `setVariableValue` |
| `VariableCell.tsx:69-77` | 300ms debounce on text input |
| `CellContainer.tsx:240-268` | Cell context menu with "Run from here" |
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

Add an `autoRunFromHere` boolean to the cell config. When enabled on a cell, any upstream variable change triggers `executeFromCell(cellIndex)` automatically — the same action as clicking "Run from here" manually.

**Why this approach:**
- **Simple mental model**: "this cell and everything below it re-runs when variables change"
- **Reuses existing execution machinery**: just calls `executeFromCell`
- **Granular control**: each cell decides independently
- **Cheap by default**: only cells the user explicitly marks will auto-run
- **Works with debounce**: text variables already debounce at 300ms, so auto-run won't fire on every keystroke

#### Config change

```typescript
// In notebook-types.ts
export interface CellConfigBase {
  name: string
  type: CellType
  layout: { height: number; collapsed?: boolean }
  autoRunFromHere?: boolean  // NEW: auto-execute from this cell when upstream variables change
}
```

#### Execution trigger

Since `variableValuesRef` is a mutable ref (not React state), the trigger must come from the call site:

1. `setVariableValue` already updates the ref and URL params. Add a callback parameter (`onVariableChange`) to `useNotebookVariables` that fires after the value is set.
2. `NotebookRenderer` passes a handler that finds the first `autoRunFromHere` cell downstream of the changed variable and calls `executeFromCell(index)`.

```
Variable changes → setVariableValue → onVariableChange(cellName)
  → find first autoRunFromHere cell after changed variable
  → executeFromCell(thatIndex)
```

The auto-run trigger finds the **first `autoRunFromHere` cell whose index is greater than the changed variable's index** and calls `executeFromCell` from there:
- Cells above the variable: untouched (they don't depend on it)
- Cells between variable and first autoRunFromHere: untouched (they didn't opt in)
- First autoRunFromHere cell and all below: re-executed ("from here")

#### UI: checkbox in the cell context menu

Add an "Auto-run from here" toggle in the cell's `⋮` dropdown, right below "Run from here":

```
┌──────────────────┐
│ ▶ Run from here  │
│ ☐ Auto-run from here │  ← NEW: checkbox toggle
│───────────────────│
│ 🗑 Delete cell    │
└──────────────────┘
```

When `autoRunFromHere` is enabled, show a small indicator in the cell header so the user can see at a glance which cells are reactive.

#### Debounce considerations

The existing 300ms debounce in `VariableCell.handleTextChange` (line 69-77) provides adequate protection. No additional debounce is needed because:
- Text variables: already debounced at 300ms before `onValueChange` fires
- Combobox variables: discrete selection, no debounce needed
- Expression variables: set once after execution, no rapid changes
- Datasource variables: discrete selection

If a new variable change arrives while auto-run is still executing, the in-flight execution is aborted and restarted (handled by existing `abortControllerRef` in `executeCell`, line 135-136).

### Alternative approaches considered

#### A. Dependency graph with selective re-execution

Build a DAG of cell dependencies and only re-execute cells that depend on the changed variable.

**Rejected**: Over-engineered for current needs. SQL dependency analysis is fragile. The "Run from here" semantic is simpler and matches existing UI. Can be added later as an optimization.

#### B. Global reactivity toggle (Marimo-style)

A notebook-level "auto-run" / "lazy" toggle for all cells.

**Rejected**: Too coarse. The user's example has both expensive remote queries (should NOT auto-run) and cheap WASM queries (should auto-run). Per-cell granularity is strictly more flexible.

#### C. Smart cost detection (auto-detect cheap vs expensive)

Auto-detect "cheap" (WASM) vs "expensive" (remote) cells and only auto-run cheap ones.

**Rejected**: Heuristic would be wrong sometimes. Implicit behavior is harder to understand than explicit opt-in.

#### D. Streamlit-style full re-run with caching

Re-run everything on every change, relying on caching.

**Rejected**: Fundamentally wrong for notebooks with expensive remote queries. The default should be no automatic execution.

## Implementation Steps

### Phase 1: Fix the bug

1. **Stabilize `rawTimeRange` memo** (`ScreenPage.tsx:101-107`)
   - Extract `searchParams.get('from')` and `searchParams.get('to')` into local variables
   - Use those as `useMemo` dependencies instead of the whole `searchParams` object

### Phase 2: Config and execution plumbing

2. **Add `autoRunFromHere` to cell config** (`notebook-types.ts`)
   - Add `autoRunFromHere?: boolean` to `CellConfigBase`

3. **Add `onVariableChange` callback to `useNotebookVariables`** (`useNotebookVariables.ts`)
   - Accept an optional `onVariableChange?: (cellName: string) => void` parameter
   - Call it in `setVariableValue` after updating the ref and URL

4. **Add auto-run trigger in `NotebookRenderer`** (`NotebookRenderer.tsx`)
   - Pass `onVariableChange` to `useNotebookVariables`
   - In the callback: find changed variable's index, find first `autoRunFromHere` cell after it, call `executeFromCell(thatIndex)`
   - Guard against re-entrance (don't auto-run while already auto-running)

### Phase 3: UI

5. **Add "Auto-run from here" toggle to cell context menu** (`CellContainer.tsx`)
   - Add `autoRunFromHere?: boolean` and `onToggleAutoRunFromHere?: () => void` props
   - Render checkbox item in dropdown menu below "Run from here"
   - Only show for cells that `canRun`

6. **Add auto-run indicator to cell header** (`CellContainer.tsx`)
   - When `autoRunFromHere` is true, show a small indicator in the header

7. **Wire toggle through `NotebookRenderer`** (`NotebookRenderer.tsx`)
   - Pass `autoRunFromHere` and `onToggleAutoRunFromHere` props to `CellContainer`
   - `onToggleAutoRunFromHere` calls `updateCell(index, { autoRunFromHere: !cell.autoRunFromHere })`

## Files to Modify

| File | Changes |
|------|---------|
| `ScreenPage.tsx` | Fix `rawTimeRange` useMemo dependencies |
| `notebook-types.ts` | Add `autoRunFromHere?: boolean` to `CellConfigBase` |
| `useNotebookVariables.ts` | Add `onVariableChange` callback parameter |
| `useCellExecution.ts` | No changes needed — reuses `executeFromCell` |
| `NotebookRenderer.tsx` | Wire `onVariableChange` → find autoRunFromHere cell → `executeFromCell` |
| `CellContainer.tsx` | Add "Auto-run" menu item and header indicator |

## Testing Strategy

### Regression tests for the bug fix (Phase 1)

Add tests in `ScreenPage.urlState.test.tsx` to ensure variable param changes never cause time range recomputation:

1. **`rawTimeRange` stability test**: Create a hook that mirrors `ScreenPageContent`'s time range computation. Set initial URL to `?from=now-1h&to=now`. Change a variable param via `setSearchParams`. Assert that `rawTimeRange.from` and `rawTimeRange.to` are identical before and after (same string references or equal values). This catches the root cause: `rawTimeRange` recomputing when only variable params change.

2. **`apiTimeRange` stability test**: Same setup, but also compute `apiTimeRange` via `getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to)`. After changing a variable param, assert that `apiTimeRange.begin` and `apiTimeRange.end` are identical. This catches the symptom: different ISO timestamps from `new Date()`.

3. **Positive control**: Verify that changing the `from` or `to` URL param DOES produce new `rawTimeRange` / `apiTimeRange` values (ensure the test isn't vacuous).

Note: the existing `useCellExecution.test.ts` already has "should not re-execute when timeRange object changes but values stay the same" (line 773-808), which validates the downstream behavior. The new tests target the upstream `ScreenPage` layer where the bug originates.

### Auto-run feature tests (Phase 2-3)

4. **Manual testing with the provided notebook:**
   - Import the processes-notebook with relative time range (`now-1h` to `now`)
   - Enable "Auto-run from here" on the `search_results` cell
   - Type in the `search` variable
   - Verify `search_results` updates after the 300ms debounce
   - Verify `processes` and `source` cells are NOT re-executed

5. **Unit tests:**
   - Test that `onVariableChange` callback fires when `setVariableValue` is called
   - Test auto-run index calculation (finding first `autoRunFromHere` cell after variable)

6. **Edge cases:**
   - Changing a variable with no downstream `autoRunFromHere` cells → nothing happens
   - Multiple `autoRunFromHere` cells → only the first one triggers (it runs "from here" covering the rest)
   - Auto-run while previous auto-run is still executing → previous execution aborted
   - Saving notebook persists `autoRunFromHere` flag
   - Loading notebook with `autoRunFromHere` flags does NOT cause double execution on initial load

## Open Questions

1. **Visual indicator style**: Should the auto-run indicator be a small icon, a text badge ("auto"), or a colored border/accent? This is a visual design decision that can be iterated on.

2. **Should expression variables trigger auto-run?** Expression variables compute a new value during execution. The current design handles this naturally: `executeFromCell` runs cells sequentially, so any expression variables between the start index and the autoRunFromHere cell would be re-evaluated.
