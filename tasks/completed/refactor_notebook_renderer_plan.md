# Refactor NotebookRenderer Plan

**Issue**: [#837](https://github.com/madesroches/micromegas/issues/837)

## Overview

Extract 6 custom hooks from the 1,154-line `NotebookRenderer.tsx` to reduce it to ~500 lines of orchestration + JSX. Each hook owns a distinct concern and follows the patterns established by `useNotebookVariables`, `useCellExecution`, and `useSqlHandlers`.

## Current State

`NotebookRenderer.tsx` contains 10 `useState`, 4 `useRef`, 11+ `useCallback`, 5 `useMemo`, and 4 `useEffect` hooks managing these concerns in one flat scope:

| Concern | Lines | Hooks involved |
|---|---|---|
| Save + config parsing | 265-296 | useMemo (3) |
| Time range sync | 300-323 | useRef (1), useEffect (1) |
| Variable management | 325-330 | useNotebookVariables (already extracted) |
| Auto-run guards + timers | 332-415 | useRef (2), useEffect (1), useCallback (2) |
| WASM engine loading | 345-365 | useState (2), useEffect (1) |
| Cell execution | 367-397 | useMemo (1), useCallback (2), useCellExecution (already extracted) |
| Sort-change re-execution | 422-449 | useRef (1), useEffect (1) |
| UI state (selection, modals, panel) | 451-488 | useState (6), useRef (2), useEffect (2), useCallback (1) |
| Drag and drop | 492-632 | useSensor, useMemo (1), useCallback (4), useState (3), useRef (2) |
| Cell CRUD | 634-810 | useCallback (5) |
| Rendering | 812-1022 | useMemo (1), inline functions |

Three hooks are already extracted: `useNotebookVariables` (199 lines), `useCellExecution` (356 lines), `useSqlHandlers` (70 lines).

## Design

Each new hook follows the established pattern:
- Explicit `Params` and `Result` interfaces
- Owns its own state and refs internally
- Returns callbacks and derived values
- JSDoc on the exported function

### Hook 1: `useWasmEngine`

Loads the WASM query engine asynchronously. Currently lines 345-365.

```typescript
interface UseWasmEngineResult {
  engine: NotebookQueryEngine | null
  engineError: string | null
}
```

### Hook 2: `useNotebookAutoRun`

Owns the auto-run guard ref, debounced per-cell timers, and variable-change-triggered auto-run. Currently lines 332-343 (refs/cleanup) + 399-415 (scheduleAutoRun) + the guard-and-execute pattern duplicated at lines 906-912 and 966-970 inside `renderCell`.

```typescript
interface UseNotebookAutoRunParams {
  executeFromCellByName: (name: string) => Promise<void>
}

interface UseNotebookAutoRunResult {
  /** Schedule a debounced auto-run (for config changes like SQL editing) */
  scheduleAutoRun: (cellName: string) => void
  /** Trigger immediate auto-run with re-entrance guard (for variable value changes) */
  triggerAutoRun: (cellName: string, autoRunFromHere?: boolean) => void
}
```

`triggerAutoRun` encapsulates the guard pattern currently duplicated in `renderCell`:
```typescript
// Before (duplicated in two places in renderCell):
if (autoRunningRef.current || !cell.autoRunFromHere) return
autoRunningRef.current = true
executeFromCellByName(cellName).finally(() => { autoRunningRef.current = false })

// After (single call, guard is internal to the hook):
triggerAutoRun(cellName, cell.autoRunFromHere)
```

This keeps `autoRunningRef` fully internal to the hook.

### Hook 3: `useCellSortCheck`

Tracks sort option changes on table/log cells and re-executes when they change. Currently lines 422-449.

```typescript
interface UseCellSortCheckParams {
  executionCells: CellConfig[]
  executeCellByName: (name: string) => void
}
```

No return value — this is a side-effect-only hook.

### Hook 4: `useNotebookDragDrop`

Owns all dnd-kit sensor config, drag state/refs, drop-zone computation, and the reorder/nest-into-hg logic. Currently lines 456-461 (drag state) + 492-632 (sensors, handlers).

```typescript
interface UseNotebookDragDropParams {
  cells: CellConfig[]
  notebookConfig: NotebookConfig
  onConfigChange: (config: NotebookConfig) => void
  selectedCellIndex: number | null
  setSelectedCellIndex: (index: number | null) => void
  setSelectedChildName: (name: string | null) => void
}

interface UseNotebookDragDropResult {
  sensors: ReturnType<typeof useSensors>
  hgAwareSortingStrategy: typeof verticalListSortingStrategy
  handleDragStart: (event: DragStartEvent) => void
  handleDragOver: (event: DragOverEvent) => void
  handleDragEnd: (event: DragEndEvent) => void
  activeDragId: string | null
  dragOverZone: 'before' | 'into' | 'after' | null
  dragOverHgName: string | null
}
```

### Hook 5: `useCellManager`

Owns cell add/delete/duplicate/update/collapse operations. Currently lines 634-810.

```typescript
interface UseCellManagerParams {
  cells: CellConfig[]
  notebookConfig: NotebookConfig
  existingNames: Set<string>
  onConfigChange: (config: NotebookConfig) => void
  // Execution state management
  removeCellState: (name: string) => void
  migrateCellState: (oldName: string, newName: string) => void
  // Variable management
  setVariableValue: (cellName: string, value: VariableValue) => void
  migrateVariable: (oldName: string, newName: string) => void
  removeVariable: (cellName: string) => void
  // Auto-run scheduling
  scheduleAutoRun: (cellName: string) => void
  // Selection updates
  selectedCellIndex: number | null
  setSelectedCellIndex: (index: number | null) => void
  setSelectedChildName: (name: string | null) => void
  setShowAddCellModal: (show: boolean) => void
  setDeletingCellIndex: (index: number | null) => void
}

interface UseCellManagerResult {
  handleAddCell: (type: CellType) => void
  handleDeleteCell: (index: number) => void
  handleDuplicateCell: (index: number) => void
  updateCell: (index: number, updates: Partial<CellConfig>) => void
  toggleCellCollapsed: (index: number) => void
}
```

### Hook 6: `useEditorPanelWidth`

Owns panel width state with localStorage persistence and resize handling. Currently lines 473-487.

```typescript
interface UseEditorPanelWidthResult {
  editorPanelWidth: number
  handleEditorPanelResize: (delta: number) => void
}
```

### What stays in NotebookRenderer

After extraction, NotebookRenderer becomes an orchestration shell (~500 lines):

- Props destructuring and save handler setup (lines 254-282)
- Config parsing memos (lines 284-296)
- Hook composition (wiring the 6 new hooks + 2 existing hooks together)
- Time range sync via `useTimeRangeSync({ rawTimeRange, config: notebookConfig, onConfigChange })` (replaces inline lines 300-323)
- `executeCellByName` / `executeFromCellByName` wrappers (lines 382-397)
- `handleTimeRangeSelect` callback (line 418-420)
- UI state: `selectedCellIndex`, `selectedChildName`, `showAddCellModal`, `deletingCellIndex`, `showSource` (kept as simple `useState` in the component — they're consumed by JSX)
- `getAvailableVariables` + `datasourceVariables` memo (lines 815-841)
- `renderCell` function (lines 843-1022)
- JSX return (lines 1024-1154)

The `DeleteCellModal`, `SortableCell`, and `HgEditorPanel` sub-components stay in the same file — they're already well-isolated and small.

## Implementation Steps

### Phase 1: Extract side-effect hooks (low coupling)

1. **Create `useWasmEngine.ts`** — move lines 345-365
   - Move `loadWasmEngine` import
   - Re-export `NotebookQueryEngine` type (or import it from `useCellExecution`)
   - In NotebookRenderer, replace the useState/useEffect with `const { engine, engineError } = useWasmEngine()`

2. **Create `useEditorPanelWidth.ts`** — move lines 473-487
   - Move constants `EDITOR_PANEL_MIN_WIDTH`, `EDITOR_PANEL_MAX_WIDTH`, `EDITOR_PANEL_DEFAULT_WIDTH`
   - In NotebookRenderer, replace with `const { editorPanelWidth, handleEditorPanelResize } = useEditorPanelWidth()`

3. **Create `useNotebookAutoRun.ts`** — move lines 332-343 + 399-415 + guard pattern from lines 906-912, 966-970
   - Takes `executeFromCellByName` (the ref-wrapped version)
   - Returns `scheduleAutoRun` (debounced, for config changes) and `triggerAutoRun` (immediate, for variable changes)
   - `autoRunningRef` stays internal to the hook
   - Update `renderCell` to replace inline guard-and-execute blocks with `triggerAutoRun(cellName, cell.autoRunFromHere)`

4. **Replace inline time range sync with `useTimeRangeSync`** — delete lines 300-323
   - Replace with `useTimeRangeSync({ rawTimeRange, config: notebookConfig, onConfigChange })`
   - Import from existing `./useTimeRangeSync`

5. **Create `useCellSortCheck.ts`** — move lines 422-449
   - Takes `executionCells` and `executeCellByName`
   - Side-effect only, no return value

6. Run `yarn test` and `yarn type-check` to verify no regressions.

### Phase 2: Extract stateful hooks (medium coupling)

7. **Create `useNotebookDragDrop.ts`** — move lines 456-461 + 492-632
   - Owns `activeDragId`, `dragOverZone`, `dragOverHgName` state and refs
   - Owns `sensors`, `hgAwareSortingStrategy`, `computeHgZone`
   - Owns `handleDragStart`, `handleDragOver`, `handleDragEnd`
   - Selection index updates after reorder are handled via the params callbacks

8. **Create `useCellManager.ts`** — move lines 634-810
   - Owns `handleAddCell`, `handleDeleteCell`, `handleDuplicateCell`, `updateCell`, `toggleCellCollapsed`
   - All the hg child rename/delete logic moves here
   - Auto-run scheduling for config changes moves here
   - Note: `removeCellState` already calls `engine?.deregister_table` internally, so the explicit `engine.deregister_table` calls in `handleDeleteCell`/`updateCell` are redundant double-deregisters. Drop the explicit calls and remove `engine` from this hook's params.

9. Run `yarn test` and `yarn type-check` to verify no regressions.

### Phase 3: Update tests

10. **Update `NotebookRenderer.test.tsx`**
   - Existing integration tests should continue to pass (hooks are internal implementation details)
   - Verify all tests pass without modification — the component's external behavior hasn't changed

11. **Add unit tests for extracted hooks** (optional, lower priority)
    - `useWasmEngine` — mock `loadWasmEngine`, verify engine state
    - `useCellManager` — test add/delete/duplicate/update with `renderHook`
    - `useNotebookDragDrop` — test reorder and nest-into-hg logic

## Files to Modify

**New files:**
- `analytics-web-app/src/lib/screen-renderers/useWasmEngine.ts`
- `analytics-web-app/src/lib/screen-renderers/useEditorPanelWidth.ts`
- `analytics-web-app/src/lib/screen-renderers/useNotebookAutoRun.ts`
- `analytics-web-app/src/lib/screen-renderers/useCellSortCheck.ts`
- `analytics-web-app/src/lib/screen-renderers/useNotebookDragDrop.ts`
- `analytics-web-app/src/lib/screen-renderers/useCellManager.ts`

**Modified files:**
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` — replace inline logic with hook calls

## Trade-offs

**Single flat params object for `useCellManager`**: This hook has ~13 fields in its params interface because cell CRUD touches execution state, variables, and UI selection. This matches the established pattern — `useCellExecution` and `useSqlHandlers` both use a single flat params interface. Grouping into sub-objects (e.g., `variableHandlers`, `uiState`) would add indirection without benefit since they're all wired from the same scope in NotebookRenderer. The flat `UseCellManagerParams` interface documents the full dependency surface in one place, which makes it obvious this hook is the most coupled — accurately reflecting the nature of cell CRUD.

**Time range sync uses existing `useTimeRangeSync`**: The `useTimeRangeSync` hook already exists and is used by 4 other renderers (TableRenderer, ProcessListRenderer, MetricsRenderer, LocalQueryRenderer). NotebookRenderer's inline version is identical — replacing it with a one-liner keeps the codebase consistent.

**`renderCell` stays inline**: At 180 lines it's the largest remaining piece, but it's a pure render function consuming many values from the hook composition scope. Extracting it would require passing 15+ arguments or creating a render context. It reads clearly in place.

**`showSource` state stays in component**: The ESC-key effect and JSON view are purely presentational concerns tied to JSX — not worth a hook.

## Testing Strategy

- All existing `NotebookRenderer.test.tsx` tests (599 lines) must pass unchanged — the refactoring is internal, the component API doesn't change
- Run `yarn type-check` after each phase to catch import/type issues early
- Run `yarn lint` to verify no new warnings
- Manual smoke test: open a notebook with mixed cell types (table, chart, variable, hg group), verify add/delete/duplicate/drag/reorder/execute all work
