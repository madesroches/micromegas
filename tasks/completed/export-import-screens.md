# Export / Import Screens — Implementation Plan

## Status: Completed

## Overview

Add an admin section to the web app with two features: exporting screens to a JSON file and importing screens from a JSON file. The admin section lives under `/admin` with a wrench icon pinned to the bottom of the sidebar. No auth guard required, but visually separated from the main navigation.

## Export Format

A JSON file containing an array of screen objects with their full config:

```json
{
  "version": 1,
  "exported_at": "2026-01-27T12:00:00Z",
  "screens": [
    {
      "name": "my-screen",
      "screen_type": "notebook",
      "config": { ... }
    }
  ]
}
```

The `created_by`, `updated_by`, `created_at`, `updated_at` fields are excluded from the export — they will be set fresh on import.

## Tasks

### 1. Frontend: Add export/import helper functions

**File:** `analytics-web-app/src/lib/screens-api.ts`

No new backend endpoints needed — the existing CRUD API is sufficient.

Add helper functions:

- `buildScreensExport(screens: Screen[]): string` — takes selected screens, strips timestamps/user fields, wraps in the export envelope, returns JSON string. Uses existing `listScreens()` data.
- `parseScreensImportFile(json: string): { version: number, screens: ExportedScreen[] }` — parses and validates an export file. Throws on invalid format.
- `importScreen(screen: ExportedScreen, onConflict: 'skip' | 'overwrite' | 'rename', existingNames: Set<string>): Promise<ImportScreenResult>` — imports a single screen using the existing API:
  - **new screen**: call `createScreen()`
  - **conflict + skip**: return skipped status
  - **conflict + overwrite**: call `updateScreen()`
  - **conflict + rename**: call `createScreen()` with suffixed name (e.g. `-imported`, `-imported-2`)

Add `ExportedScreen` and `ImportScreenResult` types.

### 2. Frontend: Add admin routes

**File:** `analytics-web-app/src/router.tsx`

Add three lazy-loaded routes:

- `/admin` → `AdminPage` (dashboard hub)
- `/admin/export-screens` → `ExportScreensPage`
- `/admin/import-screens` → `ImportScreensPage`

### 3. Frontend: Update sidebar with admin icon at bottom

**File:** `analytics-web-app/src/components/layout/Sidebar.tsx`

- Split the sidebar into two sections: main nav (top) and admin nav (bottom, pushed down with `mt-auto` or flex spacer)
- Add a wrench icon linking to `/admin` below a separator, pinned to the bottom of the sidebar
- The admin icon follows the same active-state pattern as existing nav items, matching on `/admin` paths

### 4. Frontend: Admin dashboard page

**File:** `analytics-web-app/src/routes/AdminPage.tsx`

- Page header: "Admin" with subtitle
- Two clickable cards in a grid:
  - **Export Screens** — download icon, links to `/admin/export-screens`
  - **Import Screens** — upload icon, links to `/admin/import-screens`
- No auth guard wrapper (feature is unprotected)
- Uses `PageLayout` wrapper for consistent chrome

### 5. Frontend: Export screens page

**File:** `analytics-web-app/src/routes/ExportScreensPage.tsx`

- Breadcrumb: Admin > Export Screens
- Fetches all screens with `listScreens()` on mount
- Table with checkboxes, search filter, select all / deselect all
- Right-side summary panel showing: selected count, breakdown by type, download button
- Download button calls `buildScreensExport()` with selected screens, creates a Blob, and triggers a browser file download via a temporary anchor element
- Disabled state on download button when nothing is selected

### 6. Frontend: Import screens page (wizard)

**File:** `analytics-web-app/src/routes/ImportScreensPage.tsx`

Three-step wizard:

**Step 1 — Upload:**
- Drag-and-drop zone + click to browse
- Accepts `.json` files only
- On file select, parse JSON client-side to extract screen list
- Validate the file has `version` and `screens` fields
- Show error if file is invalid

**Step 2 — Review & Select:**
- File info bar (filename, screen count, conflict count)
- Table of screens from the file with checkboxes
- For each screen, check against existing screens (fetched via `listScreens()`) and show "New" or "Exists" badge
- Per-row conflict resolution dropdown (skip / overwrite / rename) for existing screens
- Back and Continue buttons

**Step 3 — Confirm:**
- Summary panel: file name, screens to import, new count, conflict actions
- Back and Import Now buttons
- On import, call `importScreen()` for each selected screen with the chosen conflict strategy
- Show success/error result, then link back to admin or to the screens page

### 7. Tests

- **Frontend:** Add tests for the export/import helper functions (buildScreensExport, parseScreensImportFile, importScreen) and basic render tests for the three new pages.

## File Summary

| File | Action |
|------|--------|
| `analytics-web-app/src/lib/screens-api.ts` | Add export/import helper functions |
| `analytics-web-app/src/router.tsx` | Add `/admin` routes |
| `analytics-web-app/src/components/layout/Sidebar.tsx` | Add admin icon at bottom |
| `analytics-web-app/src/routes/AdminPage.tsx` | New — admin dashboard |
| `analytics-web-app/src/routes/ExportScreensPage.tsx` | New — export page |
| `analytics-web-app/src/routes/ImportScreensPage.tsx` | New — import wizard |
