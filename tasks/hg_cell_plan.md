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

**Within hg reorder:** Children can be reordered horizontally within the group. This requires a nested `DndContext` + `SortableContext` with `horizontalListSortingStrategy`. Since dnd-kit supports nested contexts, this works by stopping propagation at the child level.

**Cross-group drag (drag in / drag out):** Cells can be dragged in and out of hg groups with simplified fixed-position rules:

- **Drag IN** (top-level cell → hg group): When a top-level cell is dragged over an hg body, the group highlights as a drop target. On drop, the cell is **appended as the last child** of the group (removed from top-level).
- **Drag OUT** (hg child → top-level): When a child is dragged outside its parent hg body, it becomes a **top-level cell inserted immediately after the group**.

This avoids collision detection ambiguity — no need to determine insertion position within a container. The hg body is registered as a droppable via `useDroppable` from dnd-kit. On `handleDragEnd`, check `over.id` to determine if the drop target is an hg droppable zone; if so, append to that group's children. For drag-out, detect that a child's drag ended outside its parent hg and insert after the group.

**Constraints:**
- Cannot drag an hg cell into another hg (no nesting)
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
   - Update `cleanupVariableParams` if it iterates cells (check for hg children)

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
   - Update `handleDeleteCell` to clean up all children of deleted hg cells
   - Update `handleDuplicateCell` to uniquify all child names

6. **`useCellExecution.ts`**
   - No changes needed (receives flat list, keyed by name)

### Phase 3: Editor Panel

7. **`CellEditor.tsx`**
   - Handle hg cells: when `cell.type === 'hg'`, pass through to `HorizontalGroupCellEditor`
   - Add `selectedChildIndex` and `onChildSelect` props for navigating between group and child editors

### Phase 4: Child Management (within hg)

8. **`HorizontalGroupCellEditor`** (in `HorizontalGroupCell.tsx`)
   - Add child button (add-cell modal minus 'hg' option)
   - Remove child (with confirmation)
   - Reorder children via up/down arrow buttons in the children list (moves left/right in the horizontal layout)
   - Click child → switch editor to child config

### Phase 5: Cross-Group Drag & Drop

9. **`NotebookRenderer.tsx`**
   - Add `useDroppable` on each hg cell body (droppable id: `hg-drop-${cell.name}`)
   - Update `handleDragEnd` with two new branches:
     - If `over.id` starts with `hg-drop-`: remove dragged cell from source, append to target hg's children
     - If dragged cell is an hg child and dropped outside parent hg: remove from hg children, insert after parent group in top-level list
   - Add constraint: skip drop if dragged item is type `hg`
   - Add CSS highlight on hg body when `isOver` is true (blue border glow)

10. **`HorizontalGroupCell.tsx`**
    - Nested `DndContext` + `SortableContext` with `horizontalListSortingStrategy` for within-group reordering
    - Child drag handles for horizontal reordering

## Files to Modify

| File | Action | Description |
|------|--------|-------------|
| `notebook-types.ts` | Modify | Add `'hg'` to CellType, add `HorizontalGroupCellConfig`, update `CellConfig` union |
| `notebook-utils.ts` | Modify | Add `flattenCellsForExecution` utility |
| `cells/HorizontalGroupCell.tsx` | Create | Renderer, editor, metadata |
| `cell-registry.ts` | Modify | Import and register hg metadata |
| `NotebookRenderer.tsx` | Modify | Flatten cells for execution, selection model, hg rendering, cross-group drag |
| `CellEditor.tsx` | Modify | Handle hg type, child navigation |

## Trade-offs

**Flatten at render vs. flatten in hook:**
Flattening in NotebookRenderer before passing to `useCellExecution` keeps the hook unchanged and avoids coupling it to the nesting concept. The downside is index translation logic in NotebookRenderer, but this is manageable with a name→index map.

**Shared height vs. per-child height:**
The issue specifies children share the parent hg cell's height. This is simpler (one resize handle for the group) and produces more uniform rows. Per-child height would allow mixed sizes but complicates the layout.

**Selection model (two indices vs. cell name):**
Using `selectedCellIndex + selectedChildIndex` is minimal change over the current model. An alternative is switching to `selectedCellName: string | null` which would be cleaner but requires changing more call sites. The two-index approach was chosen for smaller diff.

**Fixed-position cross-group drag:**
Dragging in always appends as last child; dragging out always inserts after the group. This avoids collision detection ambiguity (no need to determine where within a container the cell lands). Users can fine-tune order within the group via horizontal drag reordering after the cell lands. Full positional cross-container drag would require custom collision detection and `handleDragOver` live preview — significantly more complex for marginal UX gain.

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
   - Drag a top-level cell onto an hg body — verify it becomes the last child
   - Drag a child out of an hg — verify it appears after the group in top-level
   - Drag an hg cell onto another hg — verify it is rejected (no nesting)
   - Drag the last child out — verify group shows empty state
   - Reorder children within an hg via horizontal drag
   - Collapse/expand hg — verify children hidden/shown
   - Resize hg — verify children share height
5. **Existing tests:** `yarn test` still passes

## Open Questions

1. **Child variable cells:** Should variable cells be allowed as hg children? The issue says "Each child is a full CellConfig" which implies yes. Variables in a horizontal group would be unusual but not harmful. If we allow them, their values would be scoped like any other cell in the flat execution order.

2. **Empty group behavior:** When an hg has no children, should it show a helpful empty state ("Add cells to this group") or should it be invisible? Recommend an empty state prompt.

3. **Max children:** Should there be a limit on children per group? With equal-width flex, more than 4-5 children would be very cramped. Consider a soft limit or scroll.
