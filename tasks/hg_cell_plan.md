# Horizontal Group (hg) Cell Type Plan

**GitHub Issue**: #821

## Overview

Add a new `hg` (Horizontal Group) cell type that acts as a container, arranging child cells side by side in a horizontal row. This enables dashboard-style layouts within notebooks, breaking the current vertical-only stacking.

See mockups in `tasks/hg_cell_mockups/` for visual reference.

## Current State

Notebooks are a flat vertical list of cells (`NotebookConfig.cells: CellConfig[]`). Each cell:
- Has a unique name (used as dnd-kit sort ID and variable reference key)
- Is rendered via `CellContainer` with a type-specific renderer
- Executes sequentially via `useCellExecution` (flat array iteration)
- Stores execution state in `cellStates` keyed by cell name

**Key files:**
- `notebook-types.ts` — `CellType` union, `CellConfig` discriminated union, `CellConfigBase`
- `cell-registry.ts` — `CellTypeMetadata` interface, `CELL_TYPE_METADATA` registry
- `NotebookRenderer.tsx` — Main component: rendering, dnd, execution orchestration, selection
- `useCellExecution.ts` — Cell execution loop, blocking, variable scoping
- `CellContainer.tsx` — Cell header/wrapper with drag handle, controls, menu
- `CellEditor.tsx` — Right panel cell editor

## Design

### Config Shape

```ts
// In notebook-types.ts
type CellType = ... | 'hg'

interface HorizontalGroupCellConfig extends CellConfigBase {
  type: 'hg'
  children: CellConfig[]  // child cells rendered side-by-side
}

type CellConfig = ... | HorizontalGroupCellConfig
```

Each child is a full `CellConfig` (table, chart, log, markdown, variable, etc.). Nesting hg inside hg is technically possible by the type system but not supported in the UI (add-child modal excludes 'hg').

### Execution: Flattening Children

The hg cell itself does not execute. Its children are **flattened** into the execution order so `useCellExecution` treats them as regular cells.

```
Top-level cells:     [Variable_1, HG_1{Table_1, Chart_1}, Table_2]
Execution order:     [Variable_1, Table_1, Chart_1, Table_2]
```

**Approach:** Create a `flattenCellsForExecution(cells: CellConfig[]): CellConfig[]` utility in `notebook-utils.ts`. NotebookRenderer computes `executionCells = flattenCellsForExecution(cells)` and passes this flat list to `useCellExecution` instead of `cells`.

Since `useCellExecution` stores state keyed by **cell name** (not index), this change is transparent — the hook doesn't need to know about nesting. The only adaptation needed is translating between top-level visual indices and execution indices when calling `executeCell(index)` or `executeFromCell(index)`.

**Index translation:** Build a `Map<string, number>` from cell name to execution index. When a cell's run button is clicked, look up its execution index by name. When "Run from here" is clicked on an hg cell, use the execution index of its first child.

### Variable Scoping

The flattening naturally handles variable scoping. In the flat execution list, hg children appear after all cells that precede the hg group, so they see all upstream variables correctly.

Children within the same hg group execute sequentially (left to right), so a child can reference variables from siblings to its left (if any are variable cells — uncommon but valid).

### Selection Model

Currently `selectedCellIndex: number | null` identifies the selected cell. With nested children, extend to:

```ts
const [selectedCellIndex, setSelectedCellIndex] = useState<number | null>(null)
const [selectedChildName, setSelectedChildName] = useState<string | null>(null)
```

- `selectedCellIndex` points to a top-level cell (could be an hg group) — stays index-based to preserve existing reorder/delete adjustment logic (lines 417-463)
- `selectedChildName` identifies a child within an hg by name — stable across child reorder operations
- When `selectedCellIndex` points to hg and `selectedChildName` is null → show group editor
- When `selectedChildName` is set → show child's editor, resolved via `hgCell.children.find(c => c.name === selectedChildName)`
- On child rename: update `selectedChildName` to the new name
- On child delete: clear `selectedChildName` if the deleted child was selected

Clicking a top-level cell clears `selectedChildName`. Clicking a child sets both `selectedCellIndex` (to the hg's index) and `selectedChildName`.

### Rendering

The hg cell uses `CellContainer` for its outer shell (drag handle, collapse, menu) but replaces the content area with a horizontal flex container:

```
┌─────────────────────────────────────────────────────────┐
│ ☰  ▼  [HG]  my_dashboard_row                     ▶ ··· │  ← hg header
├─────────────────────────────────────────────────────────┤
│ ┌─────────────────────┐  ┌────────────────────────────┐ │
│ │ [TABLE] cpu_data    │  │ [CHART] cpu_chart          │ │
│ │  ▶  200 rows (4KB)  │  │  ▶  200 rows (4KB)        │ │
│ ├─────────────────────┤  ├────────────────────────────┤ │
│ │                     │  │                            │ │
│ │  id | name | value  │  │  ~~~chart~~~               │ │
│ │  1  | cpu  | 42     │  │                            │ │
│ └─────────────────────┘  └────────────────────────────┘ │
├─────────────────────────────────────────────────────────┤
│ ═══════════════ resize handle ══════════════════════════ │
└─────────────────────────────────────────────────────────┘
```

Children share the parent hg cell's height (controlled by `layout.height` on the hg cell). Children take equal width (`flex: 1`).

Each child has a **compact header** showing: type badge, name, status text, run button, menu. The child header is a simplified version of `CellContainer`'s header — extracted as a reusable `ChildCellHeader` component within `HorizontalGroupCell.tsx`.

### Editor Panel

**When hg group is selected** (no child selected):
- Group name editor (shared with all cells via CellEditor)
- List of children with left/right arrow buttons for reordering and remove buttons
- "Add child" button (opens same add-cell modal, minus 'hg')

Each child row in the editor list shows: type badge, name, move-left arrow (disabled if first), move-right arrow (disabled if last), remove button.

**When a child is selected**:
- Back button → returns to group editor
- Standard cell editor for the child type (name, data source, type-specific fields)

The `HorizontalGroupCellEditor` component manages both views. When `selectedChildName` is set, it finds the child by name and delegates to the child's own `EditorComponent`.

### Drag & Drop

**Top-level vertical reorder:** The hg cell participates in the vertical `SortableContext` like any other cell. Its `id` is the hg cell name.

**Within-hg reorder:** Children can be reordered horizontally within the group. This requires a nested `DndContext` + `SortableContext` with `horizontalListSortingStrategy`. Since dnd-kit supports nested contexts, this works by stopping propagation at the child level.

**Three-zone drop targeting on hg cells:** When a top-level cell is dragged over an hg cell, the hg cell's bounding rect is divided into three vertical zones to disambiguate intent:

- **Top ~25%** → reorder before the hg (standard `arrayMove`, same as any cell)
- **Middle ~50%** → drop into the hg (remove from top-level, append as last child)
- **Bottom ~25%** → reorder after the hg (standard `arrayMove`, same as any cell)

This uses the existing `closestCenter` collision detection — no `useDroppable` needed. The `SortableContext` reports `over.id` as the hg cell name. In `onDragOver`, compute the pointer's Y position relative to the hg cell's bounding rect (via `over.rect`) to determine the active zone, and set a `dragOverZone` state (`'before' | 'into' | 'after' | null`). Visual feedback during drag:
- Top/bottom zones: blue insertion line above/below the hg cell
- Middle zone: blue border highlight on the hg body

In `handleDragEnd`, if `over.id` resolves to an hg cell:
- Top/bottom zone → `arrayMove` (normal reorder, existing code path)
- Middle zone → remove dragged cell from top-level, append to hg's `children`

For non-hg `over` targets, `handleDragEnd` behaves exactly as before (standard `arrayMove`).

**Drag OUT** (hg child → top-level): When a child is dragged outside its parent hg body, the nested `DndContext`'s `onDragEnd` detects `over === null` (no valid drop target within the nested context). It emits a callback to the parent NotebookRenderer, which removes the child from the hg's `children` and inserts it as a top-level cell immediately after the group.

**Constraints:**
- Cannot drag an hg cell into another hg (skip drop-into zone logic when dragged item is type `hg`)
- Dragging the last child out of a group leaves it empty (shows empty state prompt)

### Add Cell Modal

"Horizontal Group" appears in the existing add cell modal via `CELL_TYPE_OPTIONS` (automatic once registered in `CELL_TYPE_METADATA`).

Adding an hg cell creates it with an empty `children: []`. The user then adds children from within the group's editor panel.

### Cell Deletion / Duplication

**Delete hg:** Removes the group and all its children. Clean up all child cell states and WASM registrations.

**Duplicate hg:** Deep-clones the group including all children. Each child name gets uniquified (same `_copy` pattern). All children need unique names since they share the notebook's global name namespace.

## Implementation Steps

### Phase 1: Type System & Registry ✅

1. **`notebook-types.ts`** ✅
   - Added `'hg'` to `CellType` union
   - Created `HorizontalGroupCellConfig` interface with `children: CellConfig[]`
   - Added to `CellConfig` union

2. **`notebook-utils.ts`** ✅
   - Added `flattenCellsForExecution(cells: CellConfig[]): CellConfig[]` utility
   - Added `collectAllCellNames(cells: CellConfig[]): Set<string>` utility
   - Updated `cleanupVariableParams` to recurse into hg children when scanning for variable cells

3. **`cells/HorizontalGroupCell.tsx`** ✅ (new file)
   - `HorizontalGroupCell` renderer — flex row of children with compact headers
   - `HorizontalGroupCellEditor` — group management (child list, add/remove, child editing)
   - `AddChildModal` — cell type picker excluding 'hg'
   - `ChildCellHeader` — compact header with drag handle, type badge, name, run button, menu
   - `hgMetadata: CellTypeMetadata` — no execute method, `canBlockDownstream: false`, `icon: 'H'`, `defaultHeight: 300`

4. **`cell-registry.ts`** ✅
   - Imported and registered `hgMetadata` in `CELL_TYPE_METADATA`

### Phase 2: Execution Integration ✅

**Design deviation:** Instead of building a `Map<string, number>` for index translation, used name-based lookup functions (`executeCellByName`, `executeFromCellByName`) that scan the flat execution list by name on each call. This avoids maintaining a separate data structure that could get out of sync with the cell list.

5. **`NotebookRenderer.tsx`** ✅
   - Computed `executionCells = flattenCellsForExecution(cells)` and passed to `useCellExecution`
   - Added `executeCellByName` / `executeFromCellByName` helpers (scan by name instead of index map)
   - Added `selectedChildName` state (name-based, stable across child reorder)
   - Updated `renderCell` to render hg cells via `HorizontalGroupCell` inside `CellContainer`
   - Wired child click → `setSelectedChildName`, child run via `executeCellByName`
   - Updated `handleDeleteCell` to loop through hg children cleaning up state, variables, WASM tables
   - Updated `handleDuplicateCell` to deep-clone hg children and uniquify all child names
   - Updated `existingNames` to use `collectAllCellNames` (includes hg children)
   - Updated sort-option monitoring to recurse into hg children and use `executeCellByName`
   - Converted `scheduleAutoRun` from index-based to name-based
   - Added `getAvailableVariables` helper that also collects variables from hg children above

6. **`useNotebookVariables.ts`** ✅
   - Updated `savedDefaultsByName` to use `flattenCellsForExecution` for scanning saved cells
   - Updated `variableValues` computation to use `flattenCellsForExecution` for all cells
   - Updated `setVariableValue` delta logic to search flattened cells for current cell

7. **`useCellExecution.ts`** ✅
   - No changes needed (receives flat list, keyed by name)

### Phase 3: Editor Panel ✅

8. **`NotebookRenderer.tsx`** ✅
   - Added `HgEditorPanel` component with group name editing, children management via `HorizontalGroupCellEditor`, and delete button
   - When `selectedCell.type === 'hg'`: renders `HgEditorPanel` instead of generic `CellEditor`
   - `HorizontalGroupCellEditor` handles both group view (child list) and child editing (delegates to child's `EditorComponent`)

### Phase 4: Child Management (within hg) ✅

9. **`HorizontalGroupCellEditor`** (in `HorizontalGroupCell.tsx`) ✅
   - Add child button opens `AddChildModal` (excludes 'hg' option)
   - Remove child button on each child row
   - Reorder children via left/right arrow buttons
   - Click child name → switches editor to child config with back button

### Phase 5: Drag & Drop Integration ✅

10. **`NotebookRenderer.tsx`** ✅
    - Added `dragOverZone` state (`'before' | 'into' | 'after' | null`) and `dragOverHgName` state
    - Added `handleDragOver` handler: computes pointer Y relative to hg rect for three-zone detection
    - Updated `handleDragEnd`: middle zone removes cell from top-level and appends to hg children; top/bottom zones do standard `arrayMove`
    - Added constraint: skips 'into' zone when dragged item is type 'hg'
    - Visual feedback: `ring-2 ring-accent-link` for middle zone, `border-t-4`/`border-b-4` for top/bottom zones

11. **`HorizontalGroupCell.tsx`** ✅
    - Nested `DndContext` + `SortableContext` with `horizontalListSortingStrategy` for within-group reordering
    - Child drag handles for horizontal reordering
    - In nested `onDragEnd`: if `over === null`, calls `onChildDragOut` to extract child to top-level

### Phase 6: Verification ✅

- `yarn type-check` passes
- `yarn lint` passes (0 errors, 1 pre-existing warning)
- `yarn build` succeeds
- `yarn test` passes (716/716 tests)

## Files Modified

| File | Action | Description |
|------|--------|-------------|
| `notebook-types.ts` | Modified | Added `'hg'` to CellType, added `HorizontalGroupCellConfig`, updated `CellConfig` union |
| `notebook-utils.ts` | Modified | Added `flattenCellsForExecution`, `collectAllCellNames`, updated `cleanupVariableParams` |
| `cells/HorizontalGroupCell.tsx` | Created | Renderer, editor, child header, add-child modal, metadata |
| `cell-registry.ts` | Modified | Imported and registered hg metadata |
| `NotebookRenderer.tsx` | Modified | Flattened cells for execution, added `HgEditorPanel`, three-zone drag, name-based execution, hg rendering, selection model |
| `useNotebookVariables.ts` | Modified | Recurse into hg children for variable scanning |
| `CellEditor.tsx` | Not modified | Hg editor handled directly in NotebookRenderer via `HgEditorPanel` |

## Trade-offs

**Flatten at render vs. flatten in hook:**
Flattening in NotebookRenderer before passing to `useCellExecution` keeps the hook unchanged and avoids coupling it to the nesting concept. The downside is index translation logic in NotebookRenderer, but this is manageable with a name→index map.

**Shared height vs. per-child height:**
The issue specifies children share the parent hg cell's height. This is simpler (one resize handle for the group) and produces more uniform rows. Per-child height would allow mixed sizes but complicates the layout.

**Selection model (index + name hybrid):**
`selectedCellIndex` stays index-based for the top-level cell (preserving existing reorder/delete adjustment logic at lines 417-463). `selectedChildName` is name-based — stable across child reorder and drag operations, avoiding the bug where an index silently points to the wrong child after mutation.

**Three-zone drop targeting vs. useDroppable:**
Using `useDroppable` on the hg body would conflict with the `SortableContext` — both register overlapping rects for the same element, causing `closestCenter` to oscillate between the sortable slot and the droppable zone. The three-zone approach avoids this entirely: the sortable context handles collision detection as usual, and the zone is determined by pointer Y position relative to the hg cell's rect in `onDragOver`. Top/bottom zones reorder normally; middle zone triggers drop-into. This keeps the existing collision detection unchanged and makes intent unambiguous through spatial position.

**Drop-into appends as last child:**
When dropping into an hg via the middle zone, the cell is appended as the last child rather than inserted at a specific position. Users can fine-tune order within the group via horizontal drag reordering after the cell lands. Positional insertion would require computing drop position within the horizontal child layout during the vertical drag — significantly more complex for marginal UX gain.

**No hg-in-hg nesting:**
The type system allows it but the UI prevents it (add-child modal excludes 'hg'). This keeps the implementation simple while leaving the door open for future nesting if needed.

## Testing Strategy

1. **Type check:** `yarn type-check` passes with new types
2. **Build:** `yarn build` succeeds
3. **Lint:** `yarn lint` passes
4. **Manual testing:**
   - Add an hg cell from the modal
   - Add children (table, chart, log) to the group via editor
   - Verify children render side by side
   - Run notebook — verify children execute in order
   - Variable cells above hg are accessible to children
   - Delete an hg group — verify all children cleaned up
   - Duplicate an hg group — verify all child names uniquified
   - Drag hg in vertical list — verify reorder works
   - Drag a top-level cell to the top zone of an hg — verify it reorders before the hg
   - Drag a top-level cell to the middle zone of an hg — verify it becomes the last child
   - Drag a top-level cell to the bottom zone of an hg — verify it reorders after the hg
   - Verify visual feedback: insertion line for top/bottom zones, body highlight for middle zone
   - Drag a child out of an hg — verify it appears after the group in top-level
   - Drag an hg cell onto another hg's middle zone — verify it is rejected (no nesting, treated as reorder)
   - Drag the last child out — verify group shows empty state
   - Reorder children within an hg via horizontal drag
   - Collapse/expand hg — verify children hidden/shown
   - Resize hg — verify children share height
5. **Existing tests:** `yarn test` still passes

## Open Questions

1. ~~**Child variable cells:**~~ **Resolved: allowed.** Flattening handles scoping correctly — no restrictions needed.

2. ~~**Empty group behavior:**~~ **Resolved: show empty state prompt** ("Add cells to this group").

3. ~~**Max children:**~~ **Resolved: no hard limit.** Let `flex: 1` naturally degrade; address later if needed.
