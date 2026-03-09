# Notebook Alt+PageUp/PageDown Cell Navigation Plan

**Issue**: [#909](https://github.com/madesroches/micromegas/issues/909)

## Overview

Add Alt+PageUp and Alt+PageDown keyboard navigation to move between cells in the notebook view. Currently there is no keyboard-based cell navigation — selection is only possible via mouse click or the context menu. This makes it tedious to move through large notebooks.

## Current State

Cell selection is managed by `selectedCellIndex` state in `NotebookRenderer.tsx:357`:
```typescript
const [selectedCellIndex, setSelectedCellIndex] = useState<number | null>(null)
```

Selection happens via mouse:
- Double-click a cell header (`CellContainer.tsx:329,362,392`)
- "Edit cell" in the context menu (`CellContainer.tsx:214-221`)

There is no keyboard event handling at the notebook level. The only keyboard support is dnd-kit's `KeyboardSensor` for drag-and-drop reordering (`useNotebookDragDrop.ts`).

When a cell is selected, there is no `scrollIntoView()` call — the cell may be off-screen after selection.

### Key files
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` — orchestrator, owns `selectedCellIndex`
- `analytics-web-app/src/components/CellContainer.tsx` — cell wrapper, receives `ref` from dnd-kit
- `analytics-web-app/src/lib/screen-renderers/useNotebookDragDrop.ts` — existing keyboard sensor for drag-and-drop

## Design

### Flat navigation list

Navigation treats HG children as individual stops. A flat navigation list is built from the `cells` array:

```
Cell A          → { cellIndex: 0, childName: null }
HG Group        → (skipped — not a nav stop itself)
  ├─ Child B    → { cellIndex: 1, childName: 'B' }
  └─ Child C    → { cellIndex: 1, childName: 'C' }
Cell D          → { cellIndex: 2, childName: null }
```

The flat list is built with a `useMemo` inside the hook. Collapsed HG groups are skipped — their children are not in the DOM (CellContainer doesn't render content when collapsed), so there is nothing to scroll to:
```typescript
type NavTarget = { cellIndex: number; childName: string | null }

const navTargets: NavTarget[] = cells.flatMap((cell, i) =>
  cell.type === 'hg'
    ? cell.layout.collapsed
      ? []  // collapsed HG: children not rendered, skip
      : (cell as HorizontalGroupCellConfig).children.map(child => ({ cellIndex: i, childName: child.name }))
    : [{ cellIndex: i, childName: null }]
)
```

The current nav position is found by matching `(selectedCellIndex, selectedChildName)` against this list.

### New hook: `useNotebookKeyboardNav`

A custom hook that:
1. Maintains a ref map (`Map<string, HTMLElement>`) keyed by cell name to track each cell's DOM element
2. Builds the flat navigation list from the cells array (skipping children of collapsed HG groups)
3. Attaches a `keydown` listener on `document`
4. On Alt+PageDown: selects the next nav target and scrolls it into view
5. On Alt+PageUp: selects the previous nav target and scrolls it into view

```typescript
interface UseNotebookKeyboardNavParams {
  cells: CellConfig[]
  selectedCellIndex: number | null
  selectedChildName: string | null
  setSelectedCellIndex: (index: number | null) => void
  setSelectedChildName: (name: string | null) => void
  disabled: boolean // true when modals are open or source view is shown
}

interface UseNotebookKeyboardNavResult {
  /** Callback to register a cell's DOM element by name */
  setCellRef: (name: string, element: HTMLElement | null) => void
}
```

### Keyboard behavior

| Key | Action |
|-----|--------|
| Alt+PageDown | Select next nav target, scroll into view |
| Alt+PageUp | Select previous nav target, scroll into view |

**Why Alt modifier**: Plain PageUp/PageDown would conflict with scrolling inside editors (CodeMirror SQL editor, textareas). Ctrl+PageUp/PageDown is intercepted by browsers to switch tabs and cannot be prevented. Alt+PageUp/PageDown avoids both conflicts — CodeMirror doesn't bind these, browsers don't intercept them, and the Alt modifier makes the intent unambiguous.

**No focus guard needed**: Because the Alt modifier has no default browser behavior in inputs/textareas/contenteditable, no `e.target` check is required. The handler simply checks `e.altKey && e.key === 'PageDown'` and calls `e.preventDefault()`.

**When no cell is selected**: Alt+PageDown selects the first nav target, Alt+PageUp selects the last.

**When an HG group header is selected**: The HG group itself is not a nav target (only its children are). The current position is resolved by finding the first navTarget with `cellIndex >= selectedCellIndex`. This naturally lands on the first child of the selected group (if expanded) or the next visible cell after it. From there, Alt+PageDown/PageUp proceeds normally.

**At boundaries**: Do nothing (no wrapping). Alt+PageDown on the last target stays on it. Alt+PageUp on the first stays on it.

**Scroll behavior**: `element.scrollIntoView({ behavior: 'smooth', block: 'nearest' })` — only scrolls if the cell is outside the visible area.

### Disabled state

Navigation is disabled when:
- `showSource` is true (JSON source view is active)
- `showAddCellModal` is true
- `deletingCellIndex !== null` (delete confirmation modal is open)

These are passed as a single `disabled` boolean from NotebookRenderer.

### Ref tracking

Refs are tracked in a `Map<string, HTMLElement>` keyed by cell name (cell names are unique across the entire notebook, including HG children).

**Top-level cells**: In `renderCell`, create a combined ref callback that calls both dnd-kit's `setNodeRef` and the hook's `setCellRef`:
```typescript
const combinedRef = (el: HTMLElement | null) => {
  setNodeRef(el)             // dnd-kit's ref
  setCellRef(cell.name, el)  // our ref map
}
```

**HG children**: Add an `onChildRef` callback prop to `HorizontalGroupCellProps`. NotebookRenderer passes `setCellRef` through. Inside `HorizontalGroupCell`, `HgChildPane` creates a combined ref:
```typescript
const combinedRef = (el: HTMLElement | null) => {
  setNodeRef(el)                   // dnd-kit's ref
  onChildRef?.(child.name, el)     // our ref map
}
```

### Listener setup

The keydown listener is attached to `document` via `useEffect`. Since the Alt modifier makes the shortcut unambiguous, a global listener is safe — no container ref or `tabIndex` changes needed. The handler calls `e.preventDefault()` to suppress the browser's default Alt+PageDown behavior (switch tabs in some browsers).

## Implementation Steps

1. **Create `useNotebookKeyboardNav.ts`** in `analytics-web-app/src/lib/screen-renderers/`
   - Implement the hook with ref map, flat nav list, and document-level keydown handler
   - Build `navTargets` with `useMemo` from cells array (skip collapsed HG children)
   - Find current position by matching `(selectedCellIndex, selectedChildName)`
   - On Alt+PageDown/Alt+PageUp: compute new target, call `setSelectedCellIndex` + `setSelectedChildName`, scroll into view
   - Export `UseNotebookKeyboardNavParams`, `UseNotebookKeyboardNavResult`

2. **Wire the hook into `NotebookRenderer.tsx`**
   - Call `useNotebookKeyboardNav` with cells, selection state, and disabled flag
   - Compute `disabled` from `showSource || showAddCellModal || deletingCellIndex !== null`
   - In `renderCell`, create combined ref callbacks that call both `setNodeRef` and `setCellRef`
   - For HG cells, pass `setCellRef` as `onChildRef` to `HorizontalGroupCell`

3. **Add `onChildRef` prop to `HorizontalGroupCell`**
   - Add `onChildRef?: (name: string, el: HTMLElement | null) => void` to `HorizontalGroupCellProps`
   - Pass it through to `HgChildPane`
   - In `HgChildPane`, create a combined ref that calls both dnd-kit's `setNodeRef` and `onChildRef`

4. **Add scroll-into-view on selection change**
   - Inside the hook, use a `useEffect` that watches `selectedCellIndex` and `selectedChildName`, and calls `scrollIntoView` on the corresponding cell element from the ref map
   - This also benefits mouse-based selection (e.g., selecting via "Edit cell" menu when the cell is partially off-screen)

5. **Test and lint**
   - Run `yarn type-check` and `yarn lint` from `analytics-web-app/`

## Files to Modify

| File | Action |
|------|--------|
| `analytics-web-app/src/lib/screen-renderers/useNotebookKeyboardNav.ts` | **Create** — new hook |
| `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` | **Modify** — wire the hook, combined refs |
| `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` | **Modify** — add `onChildRef` prop, combined refs in `HgChildPane` |

## Trade-offs

### Alt+PageUp/PageDown vs. plain PageUp/PageDown vs. Arrow keys
**Chosen: Alt+PageUp/PageDown.** Arrow keys conflict with scrolling, text editing, and dnd-kit's keyboard sensor. Plain PageUp/PageDown conflict with scrolling inside the SQL editor (CodeMirror) and textareas. Ctrl+PageUp/PageDown is intercepted by browsers to switch tabs and cannot be prevented with `e.preventDefault()`. Alt+PageUp/PageDown is conflict-free — not bound by CodeMirror, not intercepted by browsers — and needs no focus guard.

### Global keydown (document) vs. container-scoped
**Chosen: Global (document-level).** The Alt modifier makes the shortcut unambiguous, so there's no risk of firing in unintended contexts. A container-scoped listener would require `tabIndex={-1}` on the scroll container and the user to click in the cell area before navigation works — the Alt modifier eliminates this usability gap.

### Hook vs. inline
**Chosen: Separate hook.** Follows the established pattern of extracting concerns into hooks (useWasmEngine, useEditorPanelWidth, useCellManager, etc.). Keeps NotebookRenderer from growing.

### scrollIntoView on all selection changes
**Chosen: Yes.** A `useEffect` on `selectedCellIndex` that scrolls the cell into view benefits both keyboard and mouse selection (e.g., selecting a cell from the context menu when it's partially off-screen). The `block: 'nearest'` option ensures it only scrolls when necessary.

## Documentation

- `mkdocs/docs/web-app/notebooks/index.md` — Add a "Keyboard Navigation" subsection under "Working with Cells" documenting Alt+PageUp/PageDown behavior.

## Testing Strategy

1. **Manual testing**:
   - Open a notebook with 10+ cells that overflow the viewport
   - Press Alt+PageDown — first cell should be selected and visible
   - Press Alt+PageDown repeatedly — selection moves down one cell at a time, scrolling as needed
   - Press Alt+PageUp — selection moves up
   - At first cell, Alt+PageUp does nothing; at last cell, Alt+PageDown does nothing
   - Click into the SQL editor in the right panel, press Alt+PageDown — cell selection should still advance (Alt modifier avoids conflict with editor scrolling)
   - Press plain PageDown in the SQL editor — editor should scroll normally, cell selection should not change
   - Open a modal (Add Cell, Delete Cell), press Alt+PageDown — nothing should happen
   - Switch to Source View, press Alt+PageDown — nothing should happen

2. **Edge cases**:
   - Empty notebook (no cells) — Alt+PageUp/PageDown do nothing
   - Single cell — both keys select it but don't go past it
   - HG cells — children are individually navigable (Alt+PageDown steps through each child in left-to-right order)
   - Empty HG group (no children) — skipped entirely in navigation
   - Collapsed HG group — children are skipped in navigation (they have no DOM elements when collapsed); navigation jumps to the next visible cell
