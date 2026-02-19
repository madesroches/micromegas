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
const [selectedChildIndex, setSelectedChildIndex] = useState<number | null>(null)
```

- `selectedCellIndex` points to a top-level cell (could be an hg group)
- `selectedChildIndex` is set when a child within an hg is clicked
- When `selectedCellIndex` points to hg and `selectedChildIndex` is null → show group editor
- When `selectedChildIndex` is set → show child's editor

Clicking a top-level cell clears `selectedChildIndex`. Clicking a child sets both indices.

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

The `HorizontalGroupCellEditor` component manages both views. When `selectedChildIndex` is set, it delegates to the child's own `EditorComponent`.

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

### Phase 1: Type System & Registry

1. **`notebook-types.ts`**
   - Add `'hg'` to `CellType` union (line 98)
   - Add `'hg'` is not a `QueryCellConfig` type — create `HorizontalGroupCellConfig` interface
   - Add `HorizontalGroupCellConfig` to `CellConfig` union (line 144)

2. **`notebook-utils.ts`**
   - Add `flattenCellsForExecution(cells: CellConfig[]): CellConfig[]` utility
   - Update `cleanupVariableParams` to recurse into hg children when scanning for variable cells

3. **`cells/HorizontalGroupCell.tsx`** (new file)
   - `HorizontalGroupCell` renderer — flex row of children with compact headers
   - `HorizontalGroupCellEditor` — group management (child list, add/remove, child editing)
   - `hgMetadata: CellTypeMetadata` — no execute method, `canBlockDownstream: false`, `icon: 'H'`, `defaultHeight: 300`

4. **`cell-registry.ts`**
   - Import and register `hgMetadata` in `CELL_TYPE_METADATA`

### Phase 2: Execution Integration

5. **`NotebookRenderer.tsx`**
   - Compute `executionCells = flattenCellsForExecution(cells)` and pass to `useCellExecution`
   - Build execution index lookup map for `executeCell` / `executeFromCell` calls
   - Add `selectedChildIndex` state
   - Update `renderCell` to render hg cells with horizontal child layout
   - Wire child click → `setSelectedChildIndex`
   - Wire child run/runFromHere through execution index lookup
   - Update `handleDeleteCell` to loop through hg children calling `removeCellState`, `removeVariable`, `engine.deregister_table` for each child
   - Update `handleDuplicateCell` to deep-clone hg children and generate unique names for each child
   - Update `existingNames` set to include child names (currently `cells.map(c => c.name)` misses children)
   - Update sort-option monitoring (`cells.forEach` loop) to recurse into hg children for table/log cells

6. **`useNotebookVariables.ts`**
   - Update cell iteration for variable defaults (currently flat `cells` scan) to also check inside hg `children`
   - Update `cells.find` for variable delta logic to also search inside hg `children`

7. **`useCellExecution.ts`**
   - No changes needed (receives flat list, keyed by name)

### Phase 3: Editor Panel

8. **`CellEditor.tsx`**
   - Handle hg cells: when `cell.type === 'hg'`, pass through to `HorizontalGroupCellEditor`
   - Add `selectedChildIndex` and `onChildSelect` props for navigating between group and child editors

### Phase 4: Child Management (within hg)

9. **`HorizontalGroupCellEditor`** (in `HorizontalGroupCell.tsx`)
   - Add child button (add-cell modal minus 'hg' option)
   - Remove child (with confirmation)
   - Reorder children via up/down arrow buttons in the children list (moves left/right in the horizontal layout)
   - Click child → switch editor to child config

### Phase 5: Drag & Drop Integration

10. **`NotebookRenderer.tsx`**
   - Add `dragOverZone` state (`'before' | 'into' | 'after' | null`) and `dragOverHgName` state
   - Add `onDragOver` handler: when `over.id` resolves to an hg cell, compute pointer Y relative to `over.rect` to determine zone (top 25% / middle 50% / bottom 25%), update `dragOverZone`; clear when over a non-hg cell
   - Update `handleDragEnd`: when `over.id` is an hg cell and `dragOverZone === 'into'`, remove dragged cell from top-level and append to hg's `children`; for `'before'`/`'after'` zones, use standard `arrayMove`
   - Add constraint: skip `'into'` zone when dragged item is type `hg`
   - Pass `dragOverZone` and `dragOverHgName` to hg cell renderer for visual feedback (insertion line or body highlight)
   - Add `onChildDragOut` callback: receives child name and parent hg name, removes child from hg `children`, inserts as top-level cell after the group

11. **`HorizontalGroupCell.tsx`**
    - Nested `DndContext` + `SortableContext` with `horizontalListSortingStrategy` for within-group reordering
    - Child drag handles for horizontal reordering
    - In nested `onDragEnd`: if `over === null` (child dragged outside bounds), call `onChildDragOut` callback to extract child to top-level
    - Visual feedback: blue insertion line (top/bottom zone) or blue border highlight (middle zone) based on passed `dragOverZone` prop

## Files to Modify

| File | Action | Description |
|------|--------|-------------|
| `notebook-types.ts` | Modify | Add `'hg'` to CellType, add `HorizontalGroupCellConfig`, update `CellConfig` union |
| `notebook-utils.ts` | Modify | Add `flattenCellsForExecution`, update `cleanupVariableParams` to recurse into hg children |
| `cells/HorizontalGroupCell.tsx` | Create | Renderer, editor, metadata |
| `cell-registry.ts` | Modify | Import and register hg metadata |
| `NotebookRenderer.tsx` | Modify | Flatten cells for execution, selection model, hg rendering, three-zone drag, `existingNames` includes child names, sort-option monitoring recurses into hg children |
| `CellEditor.tsx` | Modify | Handle hg type, child navigation |
| `useNotebookVariables.ts` | Modify | Recurse into hg children when scanning for variable cells (baseline values and delta logic) |

## Trade-offs

**Flatten at render vs. flatten in hook:**
Flattening in NotebookRenderer before passing to `useCellExecution` keeps the hook unchanged and avoids coupling it to the nesting concept. The downside is index translation logic in NotebookRenderer, but this is manageable with a name→index map.

**Shared height vs. per-child height:**
The issue specifies children share the parent hg cell's height. This is simpler (one resize handle for the group) and produces more uniform rows. Per-child height would allow mixed sizes but complicates the layout.

**Selection model (two indices vs. cell name):**
Using `selectedCellIndex + selectedChildIndex` is minimal change over the current model. An alternative is switching to `selectedCellName: string | null` which would be cleaner but requires changing more call sites. The two-index approach was chosen for smaller diff.

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

1. **Child variable cells:** Should variable cells be allowed as hg children? The issue says "Each child is a full CellConfig" which implies yes. Variables in a horizontal group would be unusual but not harmful. If we allow them, their values would be scoped like any other cell in the flat execution order.

2. **Empty group behavior:** When an hg has no children, should it show a helpful empty state ("Add cells to this group") or should it be invisible? Recommend an empty state prompt.

3. **Max children:** Should there be a limit on children per group? With equal-width flex, more than 4-5 children would be very cramped. Consider a soft limit or scroll.
