# Plan: Notebook Variable URL Delta Handling

## Status: Implemented

## Goal

Improve notebook variable handling so that:
1. When a user sets a default value, it becomes the current value
2. URL only reflects the **difference** between current value and saved value (the `defaultValue` stored in saved config)
3. If a variable's current value equals its saved value, it should NOT appear in the URL

## Implementation Summary

### 1. Pass savedConfig to the Hook ✓

**Files: `NotebookRenderer.tsx`**

Pass `savedNotebookConfig?.cells` to `useNotebookVariables`. The hook looks up saved defaults directly from the saved cells.

### 2. Delta-based URL Variable Synchronization ✓

**File: `src/lib/screen-renderers/useNotebookVariables.ts`**

- Added `savedCells` parameter to the hook
- Created `savedDefaultsByName` Map for O(1) lookup of saved defaults
- `setVariableValue` compares against saved baseline (falling back to current cell's default for new variables)
- If value matches baseline → remove from URL
- If value differs from baseline → add to URL

### 3. Setting Default Value Updates Current Value ✓

**File: `src/lib/screen-renderers/NotebookRenderer.tsx`**

When a cell's `defaultValue` changes during editing in `updateCell`:
- Call `setVariableValue(cell.name, newDefault)` to update the current value
- Delta logic automatically decides if URL needs updating

### 4. Compute Effective Values with Saved Defaults ✓

**File: `src/lib/screen-renderers/useNotebookVariables.ts`**

`variableValues` computation:
1. Start with baseline values (saved default → current cell default)
2. Override with URL values (these are the deltas from saved state)

### 5. Clean Up URL on Save ✓

**File: `src/routes/ScreenPage.tsx`**

After successful save in `handleSave`:
- Iterate through URL variables
- Remove any that now match the saved defaults
- Uses config snapshot to avoid race conditions

### 6. Simplified VariableCell Rendering ✓

**File: `src/lib/screen-renderers/cells/VariableCell.tsx`**

Simplified the component to use `value` prop as source of truth:
- `localValue` state only temporarily holds value while user is typing (for immediate UI feedback)
- When `value` prop changes externally, `localValue` resets to show the new value
- Debouncing only on output (callback to parent), not on display
- No complex sync effects or refs magic

## Edge Cases Handled

1. **New variable (unsaved)**: Not in savedCells, use current cell's defaultValue as baseline
2. **Deleted variable**: Remove from URL if present
3. **Renamed variable**: Remove old name from URL, apply delta logic to new name
4. **Combobox with SQL**:
   - If saved default exists in SQL results → select it (matches baseline, no URL param)
   - If saved default is NOT in SQL results → select first option, which differs from baseline → appears in URL
   - If URL override value is not in SQL results → fall back to first option (URL param effectively ignored, data changed)
5. **Empty string vs undefined**: Empty string is a valid value different from undefined

## Testing Scenarios

1. Create variable with default "foo" -> URL should be empty
2. Change value to "bar" -> URL should contain variable=bar
3. Change value back to "foo" -> URL should become empty
4. Save notebook -> URL remains as-is (delta from saved value)
5. After save, change value to match saved value -> URL clears that param
6. Share URL with overrides -> Recipient sees overridden values applied on top of saved values
