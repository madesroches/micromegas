# Screen Page: Discard Changes + Less Invasive "Managed by Source Control" Notice

## Overview
Two changes to the screen title bar in `ScreenPage.tsx`: (1) add a **Discard** button that reverts
unsaved edits to the saved screen (issue #1755), and (2) replace the full-width "managed by source
control" warning banner, which pushes content down whenever a git-managed screen has unsaved changes,
with a compact warning-triangle indicator next to Save.

## Current State
- `routes/ScreenPage.tsx:126` derives `hasUnsavedChanges` by comparing `screenConfig` + `rawTimeRange` to `baselineConfig`. Title bar shows `(unsaved changes)` with Diff / Save As / Save (~lines 397-436).
- Time range and variable overrides live in the URL as deltas against the saved config; they survive reload, so there is no way back short of editing the URL.
- When `screen.managed_by && hasUnsavedChanges`, a full-width bordered banner is rendered under the title (~line 443) with a "View source" link. Documented in `mkdocs/docs/web-app/notebooks/screens-as-code.md:155`.
- `components/ConfirmDialog.tsx` already provides a `variant="danger"` confirmation dialog.
- Renderers are keyed by `screen?.name` (`ScreenPage.tsx` Renderer `key`), and own URL cleanup on save (`useDefaultSaveCleanup`, `NotebookRenderer.tsx:301`).

## Design

### 1. Discard button
- Shown when `hasUnsavedChanges && !isNew`, placed next to Diff, ghost variant with error-colored text and a `Undo2` lucide icon.
- Click opens `ConfirmDialog` (`variant="danger"`, "Discard unsaved changes?", confirm label "Discard").
- On confirm:
  - `setScreenConfig(screen.config)` (baseline already equals it).
  - Clear **all** query params with `navigate(pathname, { replace: true })` so time range and every renderer's URL deltas (variables, log level, etc.) drop at once — avoids teaching ScreenPage about each renderer's param names.
  - Bump a `discardNonce` state included in the Renderer `key` so renderers holding local state remount from the saved config.
- Logic lives in one `handleDiscard` callback in `ScreenPage.tsx`; no renderer changes.

### 2. Managed-by-source-control indicator
Recommended: **Option A** — `AlertTriangle` icon button in warning color, left of Save, shown when `screen?.managed_by && hasUnsavedChanges` (same condition as the banner today). Hover/focus/click opens a small popover with the existing message and "View source" link; Esc / outside click closes. Banner removed.

Option B (confirm popup before save) is rejected as the default: it adds a click to every save and must also intercept Ctrl+S (`ScreenPage.tsx:288`). Could be added later without conflict.

Extract a small `ManagedByIndicator` component (`components/ManagedByIndicator.tsx`) taking `managedBy: string`, so ScreenPage only gains one line and the popover is reusable.

## Mockups
- `tasks/1755_discard_changes_mockups/option-a-triangle-popover.html` — triangle beside Save with popover (recommended). Quiet, no layout shift.
- `tasks/1755_discard_changes_mockups/option-b-confirm-on-save.html` — clean header, confirmation dialog on Save. Strongest guard, extra click per save.

## Implementation Steps
1. Create `components/ManagedByIndicator.tsx` (triangle button + popover, accessible `aria-label`, Esc/outside-click close).
2. `ScreenPage.tsx`: remove banner block, render `ManagedByIndicator` before Save; add Discard button, `showDiscardDialog` state, `handleDiscard`, `discardNonce` in the Renderer key; render `ConfirmDialog`.
3. Update `screens-as-code.md` banner paragraph to describe the triangle indicator.
4. Add tests (below) and a `CHANGELOG.md` entry.

## Files to Modify
- `analytics-web-app/src/routes/ScreenPage.tsx`
- `analytics-web-app/src/components/ManagedByIndicator.tsx` (new)
- `mkdocs/docs/web-app/notebooks/screens-as-code.md`
- `CHANGELOG.md`

## Trade-offs
- Clearing all params vs. only known delta params: simpler and renderer-agnostic; the cost is dropping any non-config URL state (none relevant on a screen page). Rejected: per-renderer param lists, which rot.
- Popover vs. native `title` tooltip: `title` can't contain a clickable "View source" link.

## Decisions
- Discard asks for confirmation (per issue); the Diff modal does not get a Discard button for now.

- Option A (triangle + popover) chosen over B (confirm on save). The popover opens only on hover/focus/click of the triangle — never automatically; the passive cue is accepted.

## Documentation
Update `mkdocs/docs/web-app/notebooks/screens-as-code.md` (warning banner section).

## Testing Strategy
Vitest/React Testing Library, no backend:
- Discard visible only when `hasUnsavedChanges && !isNew`; cancel leaves config untouched; confirm restores saved config and clears query params.
- `ManagedByIndicator`: renders link with `managed_by` href; popover opens on click, closes on Esc.
- ScreenPage: indicator shown only for `managed_by` + unsaved changes; banner text no longer rendered.

## Manual Verification
1. Open a screen, change the time range, click Discard → confirm → URL query string empty, time range back to saved, `(unsaved changes)` gone. Not automated: needs real router + renderer wiring visual check.
2. Open a `managed_by` screen, edit → triangle appears beside Save, popover shows message and link.

