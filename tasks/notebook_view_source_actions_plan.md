# Notebook View Source: Copy and Edit Buttons

## Overview

Add two action buttons to the notebook "View Source" panel: a **Copy** button to copy the JSON to clipboard, and an **Edit** button that makes the JSON textarea writable so users can directly edit the notebook configuration.

## Current State

The view source feature lives in `NotebookRenderer.tsx` (lines 620-638). It shows a read-only `<pre>` tag with `JSON.stringify(notebookConfig, null, 2)`. The header bar has:
- "Back to notebook" link
- "JSON" badge
- "Notebook Configuration" label
- "read-only" label

There is no way to copy the source or edit it inline. The `onConfigChange` prop already supports setting a new config object, so applying edits is straightforward.

An existing clipboard pattern exists in `CopyableProcessId.tsx` using `navigator.clipboard.writeText()` with a copied/check icon state.

## Design

### Header Bar Changes

Add two icon buttons to the header row after the existing elements:

```
← Back to notebook  [JSON]  Notebook Configuration  read-only  [Copy] [Edit]
```

When Edit is active:
```
← Back to notebook  [JSON]  Notebook Configuration  editing  [Copy] [Cancel] [Apply]
```

### Copy Button
- Uses `Copy` / `Check` icons from lucide-react (already used in the codebase)
- Copies the current textarea content to clipboard via `navigator.clipboard.writeText()`
- Shows a checkmark for 2 seconds after copying (same pattern as `CopyableProcessId`)

### Edit Mode
- New state: `const [editingSource, setEditingSource] = useState(false)`
- New state: `const [sourceText, setSourceText] = useState('')`
- When Edit is clicked: set `editingSource = true`, populate `sourceText` with current JSON
- Replace `<pre>` with `<textarea>` that binds to `sourceText`
- The "read-only" label changes to "editing"
- Edit button is replaced by Cancel and Apply buttons
- **Cancel**: resets `editingSource` to false, discards changes
- **Apply**: parses `sourceText` as JSON, validates it has a `cells` array, calls `onConfigChange()` with the result, exits edit mode
- If JSON parse fails, show an inline error message below the textarea

### Textarea Styling
- Same styling as the `<pre>`: `bg-app-card border border-theme-border rounded-lg p-4 overflow-auto text-xs font-mono text-theme-text-secondary whitespace-pre`
- Add `resize-y` for vertical resize, `min-h-[200px]`, `w-full`
- In read-only mode, keep the `<pre>` as-is (no change)

## Implementation Steps

1. Add imports: `Copy, Check, Pencil` from lucide-react (add to existing import on line 3)
2. Add state variables `editingSource` and `sourceText` near `showSource` (line 348)
3. When entering edit mode, populate sourceText: `setSourceText(JSON.stringify(notebookConfig, null, 2))`
4. Add a `jsonError` state for parse error display
5. Replace the header bar (lines 622-634) with the new layout including Copy/Edit/Cancel/Apply buttons
6. Conditionally render `<pre>` (read mode) or `<textarea>` (edit mode) for the content area
7. Implement copy handler using `navigator.clipboard.writeText()` pattern from `CopyableProcessId`
8. Implement apply handler: `JSON.parse(sourceText)` -> validate -> `onConfigChange(parsed)` -> `setEditingSource(false)`
9. Reset edit state when leaving source view (Back to notebook)

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` | Add Copy/Edit buttons and edit mode to view source panel |

## Testing Strategy

- Manual: open a notebook, click View Source, verify Copy puts JSON in clipboard
- Manual: click Edit, modify JSON, click Apply, verify notebook updates
- Manual: enter invalid JSON, click Apply, verify error message appears
- Manual: click Cancel, verify changes are discarded
- Manual: press Escape while editing, verify it goes back to notebook (existing behavior)
- Verify existing NotebookRenderer tests still pass: `cd analytics-web-app && yarn test`
