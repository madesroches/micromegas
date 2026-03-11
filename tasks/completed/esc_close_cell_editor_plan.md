# ESC to Close Notebook Cell Editor Plan

**Issue**: [#919](https://github.com/madesroches/micromegas/issues/919)

## Overview

When editing a cell in a notebook, pressing the Escape key should close/dismiss the cell editor panel. Currently there is no ESC handler for the editor — it can only be closed by clicking the X button or selecting a different cell.

## Current State

### Cell editor lifecycle

Cell selection is managed by `selectedCellIndex` / `selectedChildName` state in `NotebookRenderer.tsx:379-380`. When a cell is selected, a right-side panel opens showing either `CellEditor` (regular cells) or `HgEditorPanel` (horizontal group cells) at lines 756-801.

Closing the editor sets both state values to null:
```typescript
onClose={() => { setSelectedCellIndex(null); setSelectedChildName(null) }}
```
This pattern appears at lines 777 (HG) and 793 (regular).

### Existing keyboard handling

1. **`useNotebookKeyboardNav.ts`** — Handles Alt+PageDown/PageUp for cell navigation. Attaches a `document`-level keydown listener. Disabled when `showSource || showAddCellModal || deletingCellIndex !== null` (line 446).

2. **`NotebookSourceView.tsx:40-51`** — ESC closes the JSON source view, with a confirmation dialog if there are unsaved edits. Uses `document.addEventListener('keydown', ...)`.

3. **`SyntaxEditor.tsx:156-164`** — Handles Ctrl+Enter to run queries. No ESC handling.

4. **Modals** (ConfigDiffModal, SaveScreenDialog, AddCellModal, DeleteCellModal) — Various ESC handlers for closing dialogs.

### What's missing

No ESC handler exists to close the cell editor panel (CellEditor or HgEditorPanel).

## Design

Add ESC key handling to `useNotebookKeyboardNav`. When ESC is pressed and a cell is selected, deselect the cell (closing the editor panel). This follows the existing pattern of centralizing keyboard navigation in this hook rather than scattering handlers across components.

### Key considerations

**Focus context**: ESC should close the editor even when focus is inside a textarea or input within the editor panel. The handler listens on `document`, so it catches all keydown events regardless of focus. This matches the behavior of `NotebookSourceView` which also closes on ESC from any focus context.

**No confirmation needed**: Unlike NotebookSourceView (which has unsaved JSON edits), cell editor changes are applied immediately via `onUpdate` callbacks — there is no pending unsaved state to lose. ESC simply deselects the cell.

**Disabled state**: The existing `disabled` flag already prevents keyboard handling when modals are open or source view is active. ESC should respect this same flag to avoid interfering with modal ESC handlers.

**No conflict with SyntaxEditor**: SyntaxEditor doesn't handle ESC and has no behavior that would conflict. The browser's default ESC behavior in textareas (nothing meaningful) won't be affected.

## Implementation Steps

1. **Add ESC handling to `useNotebookKeyboardNav.ts`**
   - In the existing `handler` function, add a check for `e.key === 'Escape'` (before the existing `e.altKey` check)
   - When ESC is pressed and `selectedCellIndex !== null`, call `setSelectedCellIndex(null)` and `setSelectedChildName(null)`
   - Call `e.preventDefault()` to suppress any default behavior

2. **Test and lint**
   - Run `yarn type-check` and `yarn lint` from `analytics-web-app/`

## Files to Modify

| File | Action |
|------|--------|
| `analytics-web-app/src/lib/screen-renderers/useNotebookKeyboardNav.ts` | **Modify** — add ESC handler |

## Trade-offs

### Hook vs. NotebookRenderer useEffect
**Chosen: Hook.** The keyboard nav hook already manages document-level keydown events and receives the same `disabled` flag and selection setters. Adding ESC here keeps keyboard behavior centralized rather than adding another `useEffect` to NotebookRenderer.

### Global keydown vs. editor-scoped
**Chosen: Global (document-level).** Matches the existing pattern in the hook and in NotebookSourceView. Since the `disabled` flag prevents conflicts with modals, and cell edits are applied immediately (no unsaved state), there's no risk of unintended dismissal.

### Confirmation dialog
**Not needed.** Cell editor changes are applied immediately via `onUpdate`. There's nothing to discard.

## Testing Strategy

1. **Manual testing**:
   - Open a notebook, select a cell to open the editor panel
   - Press ESC — editor panel should close, no cell selected
   - Click into the SQL textarea in the editor, press ESC — editor should still close
   - Click into the cell name input, press ESC — editor should still close
   - Open Add Cell modal, press ESC — modal should close (not the editor behind it)
   - Open Delete Cell confirmation, press ESC — confirmation should close (not the editor)
   - Switch to Source View, press ESC — source view should close (not the editor)
   - With no cell selected, press ESC — nothing should happen

2. **Edge cases**:
   - HG cell with child selected — ESC should deselect both `selectedCellIndex` and `selectedChildName`
   - Alt+PageDown to select a cell, then ESC to deselect — should work correctly in sequence
