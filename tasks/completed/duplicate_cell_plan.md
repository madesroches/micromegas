# Duplicate Cell Plan

**Status**: Completed — PR #834, closes #832

## Overview

Add a "Duplicate cell" action to the notebook cell dropdown menu, allowing users to quickly clone an existing cell with all its configuration. The duplicate is inserted immediately after the source cell with a unique name and idle execution state.

## Current State

Notebook cells are managed in `NotebookRenderer.tsx` with these operations:
- **Add**: `handleAddCell` (line 431) creates a blank cell via `createDefaultCell()` and appends it
- **Delete**: `handleDeleteCell` (line 443) removes by index and cleans up state
- **Update**: `updateCell` (line 470) applies partial config updates

The cell dropdown menu lives in `CellContainer.tsx` (lines 221-270) and currently has three actions: "Run from here", "Auto-run from here" toggle, and "Delete cell".

Cell names must be unique within a notebook (used as dnd-kit IDs and variable references). The `createDefaultCell` function in `cell-registry.ts:209` shows the naming pattern: base name with `_N` suffix for uniqueness.

## Design

### Duplicate Logic

Deep-copy the source cell's config using structured clone, then:
1. Generate a unique name by appending `_copy` (or `_copy_N`) suffix
2. Insert the clone immediately after the source cell
3. Select the new cell in the editor panel
4. New cell starts with idle execution state (no data copied)

### Name Generation

```
base = sourceName + "_copy"
if base exists: base + "_2", "_3", ...
```

This mirrors the existing `createDefaultCell` naming pattern but uses the source cell name as the base.

### UI Addition

Add a "Duplicate cell" item to the dropdown menu in `CellContainer.tsx`, between the auto-run toggle and the delete action. Uses the `Copy` icon from lucide-react.

## Implementation Steps

1. **Add `onDuplicate` prop to `CellContainer`** (`analytics-web-app/src/components/CellContainer.tsx`)
   - Add `onDuplicate?: () => void` to `CellContainerProps` interface
   - Add a new `DropdownMenu.Item` with `Copy` icon between auto-run and delete items
   - Import `Copy` from lucide-react

2. **Add `handleDuplicateCell` to `NotebookRenderer`** (`analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`)
   - Create a `handleDuplicateCell(index: number)` callback that:
     - Deep-copies the cell config via `structuredClone`
     - Generates a unique name using `existingNames`
     - Inserts the clone at `index + 1`
     - Calls `onConfigChange` with the new config
     - Sets `selectedCellIndex` to `index + 1`

3. **Wire up the callback in `renderCell`** (`NotebookRenderer.tsx`)
   - Pass `onDuplicate={() => handleDuplicateCell(index)}` to `CellContainer`

## Files to Modify

- `analytics-web-app/src/components/CellContainer.tsx` — add `onDuplicate` prop and menu item
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` — add `handleDuplicateCell` and wire it up

## Trade-offs

**Insert after vs. append to end**: Inserting after the source cell is more intuitive since the user can see the copy next to the original. Appending to the end (like "Add Cell") would lose that context.

**Copy execution state**: Not copying execution state (data, status) is intentional — the duplicated cell may reference different upstream variables depending on its new position, and stale data would be confusing. Users can run the cell when ready.

**Name format**: Using `_copy` suffix rather than `(copy)` to keep names as valid identifiers, consistent with the existing `_N` naming convention.

## Testing Strategy

- Manual: duplicate each cell type (table, chart, log, markdown, variable, etc.) and verify config is fully copied
- Manual: duplicate a cell and verify the name is unique
- Manual: duplicate a cell when a `_copy` name already exists — verify `_copy_2` is generated
- Manual: verify the duplicated cell starts in idle state with no data
- Manual: verify variable cells are duplicated correctly (config only, no variable value duplication)
- Verify existing tests still pass with `yarn test`
