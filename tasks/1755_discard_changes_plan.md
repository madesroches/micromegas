# Screen Page: Discard Changes + Less Invasive "Managed by Source Control" Notice

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1755

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
- Renderers are keyed by `screen?.name` (`ScreenPage.tsx` Renderer `key`), and own URL cleanup on save (`useDefaultSaveCleanup` in `url-cleanup-utils.ts` for most renderers; `NotebookRenderer`'s own save wrapper around line 302).

## Design

### 1. Discard button
- Shown when `hasUnsavedChanges && !isNew`, `disabled={isSaving}` (save completion would otherwise undo the discard), placed next to Diff, `variant="ghost" size="sm" className="gap-1 text-accent-error"` (matching the Diff button) with a `Undo2` lucide icon.
- Click opens `ConfirmDialog` (`variant="danger"`, "Discard unsaved changes?", confirm label "Discard").
- On confirm, `handleDiscard` closes the dialog, calls `setSaveError(null)` (so a stale save error does not stay rendered), clears **all** query params with `navigate(location.pathname, { replace: true })` (`location` from `useLocation()`; `window.location.pathname` would double the `BrowserRouter` basename) so time range and every renderer's URL deltas (variables, log level, etc.) drop at once, and sets a `pendingDiscard` flag.
- A `useEffect` on `[pendingDiscard, location.search]` completes the discard once `location.search === ''`: `setScreenConfig(baselineConfig)`, `setDiscardNonce(n => n + 1)` (included in the Renderer `key` so renderers holding local state remount from the saved config), `setPendingDiscard(false)`. Not `screen.config`: after a save `screen` holds the server response while the baseline holds the client object, and JSONB reorders keys, which the order-sensitive `JSON.stringify` in `hasUnsavedChanges` would report as a change.
- Why deferred: `BrowserRouter` (`main.tsx` passes no `useTransitions={false}`) applies location updates inside `React.startTransition` (`react-router` `lib/dom/lib.js`), so urgent state updates made in the same handler would commit while `useSearchParams` still returns the old query. The remounted renderer would seed from the old URL (`useNotebookVariables` mount effect, `LogRenderer` level/limit/search) and write the old deltas back. Gating the remount on the committed location avoids depending on React transition-lane batching.
- Logic lives in `handleDiscard` plus the one effect in `ScreenPage.tsx`; no renderer changes.

### 2. Managed-by-source-control indicator
Recommended: **Option A** — `AlertTriangle` icon button in warning color, left of Save, shown when `screen?.managed_by && hasUnsavedChanges` (same condition as the banner today). Hover/focus/click opens a small popover with the existing message and "View source" link; Esc / outside click closes. Banner removed.

Extract a small `ManagedByIndicator` component (`components/ManagedByIndicator.tsx`) taking `managedBy: string`, so ScreenPage only gains one line and the popover is reusable.

## Mockups
- `tasks/1755_discard_changes_mockups/option-a-triangle-popover.html` — triangle beside Save with popover (recommended). Quiet, no layout shift.
- `tasks/1755_discard_changes_mockups/option-b-confirm-on-save.html` — clean header, confirmation dialog on Save. Strongest guard, extra click per save.

## Implementation Steps
1. Create `components/ManagedByIndicator.tsx` (triangle button + popover, accessible `aria-label`, Esc/outside-click close).
2. `ScreenPage.tsx`: remove banner block, render `ManagedByIndicator` before Save; add Discard button, `showDiscardDialog` state, `handleDiscard` (including `setSaveError(null)`), `discardNonce` in the Renderer key; render `ConfirmDialog`.
3. Update `screens-as-code.md` banner paragraph (~line 155) and the "banner is informational only" sentence (~line 161) to describe the triangle indicator; add a Discard sentence to `notebooks/index.md` (Saving or Config Diff section).
4. Add tests (below): `src/routes/__tests__/ScreenPage.discard.test.tsx` and `src/components/__tests__/ManagedByIndicator.test.tsx`, plus a `CHANGELOG.md` entry.

## Files to Modify
- `analytics-web-app/src/routes/ScreenPage.tsx`
- `analytics-web-app/src/components/ManagedByIndicator.tsx` (new)
- `analytics-web-app/src/routes/__tests__/ScreenPage.discard.test.tsx` (new)
- `analytics-web-app/src/components/__tests__/ManagedByIndicator.test.tsx` (new)
- `mkdocs/docs/web-app/notebooks/screens-as-code.md`
- `mkdocs/docs/web-app/notebooks/index.md`
- `CHANGELOG.md`

## Trade-offs
- Clearing all params vs. only known delta params: simpler and renderer-agnostic; the cost is dropping any non-config URL state (none relevant on a screen page). Rejected: per-renderer param lists, which rot.
- Popover vs. native `title` tooltip: `title` can't contain a clickable "View source" link.

## Decisions
- Discard asks for confirmation (per issue); the Diff modal does not get a Discard button for now.

- Discard remount is gated on the committed empty `location.search` (pending-discard flag + effect) rather than wrapping the updates in `startTransition`, which relies on React lane batching with the router's internal transition.

- Option A (triangle + popover) chosen over B (confirm on save). The popover opens only on hover/focus/click of the triangle — never automatically; the passive cue is accepted. B adds a click to every save and must also intercept Ctrl+S (`ScreenPage.tsx:288`).

## Testing Strategy
Vitest/React Testing Library, no backend:
- `ScreenPage.discard.test.tsx`. `src/test-setup.ts` globally mocks `useNavigate`/`useLocation`/`useSearchParams`, so this file re-mocks `react-router` with the real module (`vi.mock('react-router', async (importOriginal) => importOriginal())`) and uses `MemoryRouter` + a `useLocation` probe to assert the query string. Mocks needed: `@/lib/auth`, `useDefaultDataSource`, `@/components/layout`, `usePageTitle`, `@/lib/screens-api` (`getScreen`, `getScreenTypes`, `getDefaultConfig`, `updateScreen`), `@/lib/screen-renderers/init` (plus a stub renderer registered in `SCREEN_RENDERERS`), and `useParams`. `PerformanceAnalysisPage.test.tsx` is the reference only for the auth/layout/data-source mocks.
  - Discard visible only when `hasUnsavedChanges && !isNew`; disabled while saving; cancel leaves config untouched.
  - Confirm restores saved config, clears the query string (start from a URL with `from`/`to` and a variable param), time range returns to the saved one, and `(unsaved changes)` disappears.
  - Stub renderer that seeds local state from `useSearchParams` and writes it back in a mount effect: after Discard the stub is remounted with empty params and the location stays `''` (nothing rewritten).
  - Indicator shown only for `managed_by` + unsaved changes and precedes Save in DOM order; banner text no longer rendered.
- `ManagedByIndicator.test.tsx`: renders link with `managed_by` href; popover opens on click, mouseEnter and focus; closes on Esc and on `mouseDown(document.body)`; `mouseDown` inside the popover leaves it open.

## Manual Verification
1. With the real notebook renderer, change a variable and, on a Log screen, the level filter, click Discard → confirm → notebook variables and Log filters visibly reset to the saved values.
