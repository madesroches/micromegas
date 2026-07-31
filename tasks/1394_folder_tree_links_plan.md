# Folder/Screen Tree Items Should Behave Like Links Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1394

## Overview

In the web app's sidebar (`analytics-web-app/src/components/FolderTree.tsx`), the folder and
screen rows are `<div role="button">` elements with an `onClick` handler, not real links. Ctrl-click,
Cmd-click, middle-click, and right-click → "Open link in new tab" all do nothing, because there is
no `href` for the browser to act on — the app is fully intercepting the click. Fix: render these
rows as real anchors (via the existing `AppLink`/react-router `Link`) so standard browser link
gestures work for free, while keeping today's click/keyboard/drag-and-drop behavior unchanged.

## Current State

- `analytics-web-app/src/components/FolderTree.tsx`:
  - `renderScreen` (line 241): the whole row is a `<div role="button" tabIndex={0} draggable onClick={() => onSelectScreen(screen.name)} onKeyDown={...}>`. No nested interactive elements — icon, name, and a search-match dot are the only children.
  - `renderNode` (line 269): the whole row is a `<div role="button" tabIndex={0} onClick={() => onSelectFolder(node.path)} onKeyDown={...}>`, containing the expand chevron (icon with its own `onClick` + `stopPropagation`), the folder icon/name, a "new subfolder" `<button>` (line 311), and a folder-actions `<button>` (line 323) that opens a Rename/Delete menu. Both buttons already call `e.stopPropagation()` so they don't trigger the row's own click.
  - Root "Home" row (line 371): same `<div role="button" onClick={() => onSelectFolder('')}>` pattern, no nested buttons.
  - All three rows also carry `dropHandlers(path)` (line 226) for drag-and-drop (folders are drop targets; screens are draggable sources).
- `analytics-web-app/src/components/layout/Sidebar.tsx`:
  - `goToFolder` (line 149) builds `/screens?folder=<path>` (preserving/clearing `q` depending on whether already on the Screens page) and calls `navigate(url, { replace: isScreensPage })`, then clears the search box.
  - `goToScreen` (line 162) calls `navigate(`/screen/${name}`)` and clears the search box.
  - Both are also called imperatively (not from a click) after folder rename/delete succeed (lines 218, 220, 239), to keep the current view pointed at the folder's new location.
  - `<FolderTree onSelectFolder={goToFolder} onSelectScreen={goToScreen} .../>` (lines 338-345) is the only place `FolderTree` is rendered.
  - `AppLink` (`analytics-web-app/src/components/AppLink.tsx`) already wraps react-router's `Link` for internal navigation and is used elsewhere in `Sidebar.tsx` (lines 276, 294) for the icon nav rail.
- `analytics-web-app/src/routes/ScreensPage.tsx` (lines ~185-230) has the precedent for this exact pattern: a draggable card `<div>` that is *not* itself the link, containing an `<AppLink href={...} className="block">` around the label/icon content, with an action `<button>` rendered as a separate sibling `<div className="absolute top-3 right-2">` outside the `AppLink` — avoiding nesting a real `<button>` inside an `<a>`.
- `FolderBreadcrumb.tsx` has the same `div[role=button]` pattern for its path segments; it is a separate component from `FolderTree` and out of scope here (see Open Questions).
- No `jsx-a11y` ESLint plugin is configured (`analytics-web-app/eslint.config.*`... — `package.json` lists only `@typescript-eslint`), so nested-interactive-content isn't lint-enforced either way, but the `ScreensPage` precedent is followed anyway for consistency.
- `src/components/__tests__/FolderTree.test.tsx` only tests the pure tree-building helpers (`buildFolderTree`, `ancestorPaths`, etc.) — no render/interaction tests exist yet for the row markup.

## Design

Split each row into:
1. An **outer container** that keeps today's layout, hover/selected styling, and drag-and-drop handlers (unchanged).
2. An **`AppLink`** wrapping the non-button visual content (chevron icon, folder/file icon, name, search-match dot) with a real `href`, so the browser sees an actual link. Its `onClick` keeps only the non-navigation side effect (clearing the sidebar search box) — navigation itself is handled by the anchor natively.
3. For folder rows only: the "new subfolder" and "folder actions" `<button>`s stay **outside** the `AppLink` as siblings (same structural fix `ScreensPage` already uses), so no `<button>` is ever nested inside an `<a>`.

`Sidebar.tsx` gains two pure href-builder functions (no navigation side effects) that `FolderTree` uses to compute each row's `href`, plus the existing `isScreensPage` flag (for the folder `Link`'s `replace` behavior, matching `goToFolder`'s current push/replace choice):

```
buildFolderUrl(path: string): string   // extracted from goToFolder's URL-building; reused by goToFolder itself
screenHref(name: string): string       // `/screen/${name}`
```

`goToFolder` keeps calling `navigate(...)` — it's still needed for the two imperative call sites
(rename/delete side effects at lines 218, 220, 239) — but now delegates URL construction to
`buildFolderUrl` instead of duplicating it, so there's one source of truth for the folder URL shape.

`FolderTree`'s props gain:
- `folderHref: (path: string) => string`
- `folderNavReplace: boolean` (passed straight through to `AppLink`'s `replace` prop for folder/Home links)
- `screenHref: (name: string) => string`

`onSelectFolder`/`onSelectScreen` keep their existing signatures and are still passed through — they
now only run the "clear search box" side effect (`AppLink`'s `onClick`), since navigation is the
anchor's job. They still fire before the browser-native default action is decided (react-router's
`Link` calls the supplied `onClick` first, then — for an unmodified left click — calls
`preventDefault()` and navigates client-side; for Ctrl/Cmd/middle-click it leaves the event alone
and the browser opens a new tab natively). No `preventDefault()` call is added anywhere, so this
"clear search" side effect still runs even when a row is opened in a background tab — a pre-existing
tradeoff, see Trade-offs.

Chevron toggle-expand, drag/drop, and both folder-menu buttons are unaffected: they already call
`e.stopPropagation()` in their own `onClick`, which stops the click from reaching the `AppLink`'s
listener at all (same mechanism that already stops it from reaching the old `div`'s listener), so no
accidental navigation on those interactions.

`role="button"` / `tabIndex={0}` / `onKeyDown={(e) => e.key === 'Enter' && ...}` are removed from
every converted row — real anchors are natively focusable, have an implicit `link` role, and already
handle Enter to activate, so this hand-rolled keyboard handling becomes redundant.

## Implementation Steps

1. **`analytics-web-app/src/components/layout/Sidebar.tsx`**
   - Extract `buildFolderUrl` from `goToFolder`'s body (the `URLSearchParams` construction), wrap in
     `useCallback` keyed on `[isScreensPage, searchParams]`. Have `goToFolder` call it instead of
     inlining the same logic.
   - Add `screenHref = useCallback((name: string) => `/screen/${name}`, [])` (or just a plain
     top-level function — no dependencies, doesn't need memoizing via component state, but match
     existing style of nearby `useCallback`s for consistency).
   - Pass the new props to `<FolderTree>` (line 338): `folderHref={buildFolderUrl}`,
     `folderNavReplace={isScreensPage}`, `screenHref={screenHref}`.

2. **`analytics-web-app/src/components/FolderTree.tsx`**
   - Import `AppLink` from `@/components/AppLink`.
   - Extend `FolderTreeProps` with `folderHref: (path: string) => string`,
     `folderNavReplace: boolean`, `screenHref: (name: string) => string`; destructure them in
     `FolderTree(...)`.
   - `renderScreen` (line 241): no nested buttons exist here, so convert the whole row into
     `<AppLink href={screenHref(screen.name)} onClick={() => onSelectScreen(screen.name)} draggable onDragStart={...} style={...} className={...}>` in place of the `<div role="button" ...>` — same children, same classes, drop `role`/`tabIndex`/`onKeyDown`.
   - `renderNode` (line 269): keep the outer `<div>` (with `dropHandlers`, `style`, and the
     selected/drop-target classes — unchanged), but replace its `role="button"`/`tabIndex`/`onClick`/
     `onKeyDown` with a child `<AppLink href={folderHref(node.path)} replace={folderNavReplace} onClick={() => onSelectFolder(node.path)} className="flex items-center gap-1.5 flex-1 min-w-0 cursor-pointer">` wrapping the chevron, folder icon, name/rename-input, and match dot (lines 289-310). The "new subfolder" `<button>` (line 311) and the folder-actions `<button>`+menu (lines 322-356) stay as siblings *after* the `AppLink`, structurally outside it — same pattern `ScreensPage.tsx` already uses to keep real `<button>`s out of an `<a>`.
   - Root Home row (line 371): same treatment as `renderScreen` — no nested buttons, so replace the
     `<div role="button" ...>` itself with `<AppLink href={folderHref('')} replace={folderNavReplace} onClick={() => onSelectFolder('')} {...dropHandlers('')} className={...}>` wrapping the `Home` icon and label.
   - Double-check `renameInput`'s existing `onClick={(e) => e.stopPropagation()}` (line 187) still
     sits inside the `AppLink` subtree so clicking into the rename `<input>` doesn't trigger
     navigation — no change needed, just confirm placement after the restructure.

3. **Tests** — `analytics-web-app/src/components/__tests__/FolderTree.test.tsx`
   - Add a new `describe('FolderTree rendering')` block (using `render`/`screen` from
     `@testing-library/react` and `MemoryRouter` from `react-router`, matching the pattern in
     `src/components/map/__tests__/EventDetailPanel.test.tsx`) that renders `FolderTree` with one
     folder and one screen and asserts:
     - the folder row and the screen row both render as `<a>` elements (`getByRole('link', ...)`)
       with the expected `href` (via `folderHref`/`screenHref` test doubles passed as props, e.g.
       `folderHref={(p) => `/screens?folder=${p}`}`, `screenHref={(n) => `/screen/${n}`}`).
     - clicking a row still calls the corresponding `onSelectFolder`/`onSelectScreen` callback (the
       side-effect path), using `MemoryRouter` so react-router's client-side navigation doesn't
       throw for the test's synthetic hrefs.
     - the "new subfolder" and folder-actions buttons are *not* inside the folder's `<a>` (e.g.
       assert `link.contains(button)` is `false`), guarding the nested-interactive-content fix.

## Files to Modify

- `analytics-web-app/src/components/FolderTree.tsx`
- `analytics-web-app/src/components/layout/Sidebar.tsx`
- `analytics-web-app/src/components/__tests__/FolderTree.test.tsx`

## Trade-offs

- **`onClick`'s search-clearing side effect still runs on a modified click (new tab).** Since we
  never call `preventDefault()`, opening a folder/screen in a new tab still clears the search box
  in the *current* tab. This is a pre-existing, minor cosmetic side effect, not a functional bug —
  fixing it would require detecting modifier/button state in `onClick` and skipping the
  side effect, which adds complexity disproportionate to a one-line UX nit. Left as-is; flagged
  here rather than silently ignored.
- **Buttons-outside-the-link structure (matching `ScreensPage.tsx`) vs. a "stretched link" overlay
  pattern** (absolutely-positioned `<a>` covering the whole row, real content layered on top via
  `pointer-events` tricks): the overlay pattern lets the *entire* row area (including the space
  visually next to the buttons) register as a link, but requires careful `pointer-events`/z-index
  bookkeeping to keep the hover-reveal buttons clickable, and this codebase has no existing use of
  it. The simpler sibling-buttons structure already used in `ScreensPage.tsx` gives the same
  ctrl-click/right-click behavior over the icon+label area (which is most of the row) with far less
  risk, so it's the better fit here.

## Testing Strategy

1. `cd analytics-web-app && yarn test` — run the extended `FolderTree.test.tsx` suite.
2. `yarn lint && yarn type-check`.
3. Manual check with `yarn dev` (or the monolith): open the Screens sidebar, ctrl-click and
   middle-click both a folder and a screen row and confirm each opens in a new tab at the right
   URL; confirm plain left-click still navigates in-place and clears the search box; confirm
   dragging a screen into a folder, expanding/collapsing a folder via the chevron, and the
   rename/delete/new-subfolder actions all still work exactly as before.

## Open Questions

- `FolderBreadcrumb.tsx` has the identical `div[role=button]` pattern for its path segments and
  would benefit from the same fix, but the issue specifically calls out "the folders bar" (the
  sidebar tree), so it's left out of scope here. Worth a follow-up issue if the same behavior is
  wanted there.
