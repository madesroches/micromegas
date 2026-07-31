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
  - `goToFolder` is also called imperatively (not from a click) after folder rename/delete succeed (lines 218, 220, 239), to keep the current view pointed at the folder's new location. `goToScreen` has no equivalent imperative call site — its only caller today is the `onSelectScreen` prop passed to `<FolderTree>`.
  - `<FolderTree onSelectFolder={goToFolder} onSelectScreen={goToScreen} .../>` (lines 338-345) is the only place `FolderTree` is rendered.
  - `AppLink` (`analytics-web-app/src/components/AppLink.tsx`) already wraps react-router's `Link` for internal navigation and is used elsewhere in `Sidebar.tsx` (lines 276, 294) for the icon nav rail.
- `analytics-web-app/src/routes/ScreensPage.tsx` (lines ~185-230) has the precedent for this exact pattern: a draggable card `<div>` that is *not* itself the link, containing an `<AppLink href={...} className="block">` around the label/icon content, with an action `<button>` rendered as a separate sibling `<div className="absolute top-3 right-2">` outside the `AppLink` — avoiding nesting a real `<button>` inside an `<a>`.
- `FolderBreadcrumb.tsx` has the same `div[role=button]` pattern for its path segments; it is a separate component from `FolderTree` and out of scope here — issue #1394's text explicitly scopes the fix to `FolderTree.tsx`'s `renderNode`/`renderScreen` and never mentions `FolderBreadcrumb.tsx`. Worth a follow-up issue if the same behavior is wanted there, but not a decision to make in this plan.
- No `jsx-a11y` ESLint plugin is configured (`analytics-web-app/eslint.config.*`... — `package.json` lists only `@typescript-eslint`), so nested-interactive-content isn't lint-enforced either way, but the `ScreensPage` precedent is followed anyway for consistency.
- `src/components/__tests__/FolderTree.test.tsx` only tests the pure tree-building helpers (`buildFolderTree`, `ancestorPaths`, etc.) — no render/interaction tests exist yet for the row markup.

## Design

Split each row into:
1. An **outer container** that keeps today's layout, hover/selected styling, and drag-and-drop handlers (unchanged).
2. An **`AppLink`** wrapping the non-button visual content (chevron icon, folder/file icon, name, search-match dot) with a real `href`, so the browser sees an actual link. Its `onClick` is a *new*, navigation-free callback — not `goToFolder`/`goToScreen` — that only clears the sidebar search box; navigation itself is handled by the anchor natively. `goToFolder`/`goToScreen` still call `navigate(...)` internally, but they are never wired to a click path again — see below for why, and for where they're still used.
3. For folder rows only: the "new subfolder" and "folder actions" `<button>`s, and — while a folder is being renamed — the rename `<input>`, all stay **outside** the `AppLink` as siblings (same structural fix `ScreensPage` already uses), so no `<button>` or focusable `<input>` is ever nested inside an `<a>`. While `renamingPath === node.path`, the `AppLink`'s label content omits the name `<span>` (the input visually replaces it in the row), and the input renders as a sibling immediately after the `AppLink`, taking over the `flex-1` slot the name `<span>` would otherwise occupy.

`Sidebar.tsx` gains two pure href-builder functions (no navigation side effects) that `FolderTree` uses to compute each row's `href`, plus the existing `isScreensPage` flag (for the folder `Link`'s `replace` behavior, matching `goToFolder`'s current push/replace choice):

```
buildFolderUrl(path: string): string   // extracted from goToFolder's URL-building; reused by goToFolder itself
screenHref(name: string): string       // `/screen/${name}`
```

`goToFolder` keeps calling `navigate(...)` — it's still needed for its three imperative call sites
(rename/delete side effects at lines 218, 220, 239) — but now delegates URL construction to
`buildFolderUrl` instead of duplicating it, so there's one source of truth for the folder URL shape.
It is no longer passed to `<FolderTree>` as `onSelectFolder`; see below. `goToScreen` has no
imperative call site at all (nothing outside `<FolderTree>`'s `onSelectScreen` prop ever calls it),
so it is deleted rather than kept.

`FolderTree`'s props gain:
- `folderHref: (path: string) => string`
- `folderNavReplace: boolean` (passed straight through to `AppLink`'s `replace` prop for folder/Home links)
- `screenHref: (name: string) => string`

`FolderTree`'s existing `onSelectFolder: (path: string) => void` / `onSelectScreen: (name: string) =>
void` props keep their type signatures, but their *meaning* changes: they are now the `AppLink`'s
`onClick` handler and therefore **must be navigation-free**. `Sidebar` no longer passes
`goToFolder`/`goToScreen` for these — it passes new closures that only clear the search box (e.g.
`() => setSearchQuery('')`), ignoring the `path`/`name` argument `FolderTree` still supplies. This is
a required correctness change, not a cosmetic one — see below.

**Why `goToFolder`/`goToScreen` cannot stay wired as the `AppLink`'s `onClick`:** react-router's
`Link` invokes the caller-supplied `onClick` *unconditionally* on every click — plain, Ctrl/Cmd,
middle, whatever. It only skips its *own* internal navigation for modified clicks; it never skips
calling `onClick`. If `onClick` were still `goToFolder`/`goToScreen` (which call `navigate(...)`),
then: (a) a plain left-click would call `navigate()` *and* let the anchor's own client-side
navigation proceed, pushing a duplicate history entry; (b) a Ctrl/Cmd-click would correctly open the
row in a new tab *and* also navigate the current tab away via the leftover `navigate()` call — which
would defeat the entire point of #1394. So the callback passed as `onClick` must be a distinct,
navigation-free function whose only job is clearing the search box; `goToFolder`/`goToScreen`'s
`navigate(...)` calls must never run on the click path.

With that split in place, `onSelectFolder`/`onSelectScreen` (now navigation-free) still fire before
the browser-native default action is decided — react-router's `Link` calls the supplied `onClick`
first, then — for an unmodified left click — calls `preventDefault()` and navigates client-side; for
Ctrl/Cmd/middle-click it leaves the event alone and the browser opens a new tab natively. No
`preventDefault()` call is added anywhere, so the "clear search" side effect still runs even when a
row is opened in a background tab — now a purely cosmetic, pre-existing tradeoff (no navigation is
involved), see Trade-offs.

Drag/drop and both folder-menu buttons are unaffected: they already call `e.stopPropagation()` in
their own `onClick`/handlers, which stops the click from reaching the `AppLink`'s listener at all
(same mechanism that already stops it from reaching the old `div`'s listener), and none of them sit
inside the `AppLink` (drag/drop handlers live on the outer row `<div>`; both buttons are kept as
siblings after the `AppLink`, per point 3 above) — so there's no anchor native-navigation default
action for their clicks to trigger in the first place.

The chevron needs one more thing, because it *is* kept nested inside the `AppLink` (wrapping the
chevron, folder icon, and match dot — see point 3 above): `stopPropagation()` alone is not enough to
stop navigation here. `stopPropagation()` only stops the click event from bubbling up to reach
react-router `Link`'s own click listener (which is where `preventDefault()` + client-side navigation
would be triggered) — but a browser's native default action for an `<a href>` (following the link) is
governed solely by whether `preventDefault()` was called on the event, completely independent of
whether propagation was stopped. So the chevron's `onClick` must call **both**:
`onClick={(e) => { e.preventDefault(); e.stopPropagation(); onToggleExpand(node.path) }}`. This is the
one case in this plan where a click target stays nested inside the `AppLink` rather than being kept
as a sibling, which is why it alone needs the extra `preventDefault()`.

`role="button"` / `tabIndex={0}` / `onKeyDown={(e) => e.key === 'Enter' && ...}` are removed from
every converted row — real anchors are natively focusable, have an implicit `link` role, and already
handle Enter to activate, so this hand-rolled keyboard handling becomes redundant.

## Implementation Steps

1. **`analytics-web-app/src/components/layout/Sidebar.tsx`**
   - Extract `buildFolderUrl` from `goToFolder`'s body (the `URLSearchParams` construction), wrap in
     `useCallback` keyed on `[isScreensPage, searchParams]`. Have `goToFolder` call it instead of
     inlining the same logic. `goToFolder` is otherwise unchanged — it still calls `navigate(...)`
     and still clears the search box — because it's still needed, unmodified, at its three
     imperative call sites (rename/delete success handlers, lines 218, 220, 239).
   - Add `screenHref = useCallback((name: string) => `/screen/${name}`, [])` (or just a plain
     top-level function — no dependencies, doesn't need memoizing via component state, but match
     existing style of nearby `useCallback`s for consistency).
   - Delete `goToScreen` entirely: once its only current caller (the `<FolderTree
     onSelectScreen={goToScreen}>` prop) is rewired below, nothing else calls it — confirmed there is
     no imperative call site analogous to `goToFolder`'s rename/delete usage.
   - Pass the new props to `<FolderTree>` (line 338): `folderHref={buildFolderUrl}`,
     `folderNavReplace={isScreensPage}`, `screenHref={screenHref}`.
   - **Rewire the click-path props** on the same `<FolderTree>` element — this is the crux of the
     fix: replace `onSelectFolder={goToFolder}` with `onSelectFolder={() => setSearchQuery('')}`, and
     replace `onSelectScreen={goToScreen}` with `onSelectScreen={() => setSearchQuery('')}`.
     `AppLink`'s `onClick` must never call `navigate()`, or every click — modified or not — would
     additionally navigate the current tab (see Design for why). `FolderTree` still invokes these
     callbacks with the folder path / screen name as an argument; both new closures simply ignore it.

2. **`analytics-web-app/src/components/FolderTree.tsx`**
   - Import `AppLink` from `@/components/AppLink`.
   - Extend `FolderTreeProps` with `folderHref: (path: string) => string`,
     `folderNavReplace: boolean`, `screenHref: (name: string) => string`; destructure them in
     `FolderTree(...)`.
   - Add a doc comment above `onSelectFolder`/`onSelectScreen` in the `FolderTreeProps` interface
     stating they are now wired to the `AppLink`'s `onClick` and must stay navigation-free — must never
     call `navigate()` — so a future change doesn't silently reintroduce the double-navigation bug
     described in Design. Keep the prop names as-is; this is a comment-only change to the interface.
   - `renderScreen` (line 241): no nested buttons exist here, so convert the whole row into
     `<AppLink href={screenHref(screen.name)} onClick={() => onSelectScreen(screen.name)} draggable onDragStart={...} style={...} className={...}>` in place of the `<div role="button" ...>` — same children, same classes, drop `role`/`tabIndex`/`onKeyDown`.
   - `renderNode` (line 269): keep the outer `<div>` (with `dropHandlers`, `style`, and the
     selected/drop-target classes — unchanged), but replace its `role="button"`/`tabIndex`/`onClick`/
     `onKeyDown` with a child `<AppLink href={folderHref(node.path)} replace={folderNavReplace} onClick={() => onSelectFolder(node.path)} draggable={false} className="flex items-center gap-1.5 cursor-pointer flex-1 min-w-0">` wrapping only the chevron, folder icon, and match dot (lines 289-304, 310) — **not** the rename input. `flex-1 min-w-0` on the `AppLink` itself is required: in the outer row's flex layout, `AppLink` is now the direct flex item, so without it the link shrinks to its content width, bunching the "new subfolder"/folder-actions buttons up against the label instead of flush-right, and long names may not truncate correctly. `draggable={false}` is also required: an `<a href>` is natively draggable, and without this a folder/Home row drag would populate `dataTransfer`'s `text/plain` with the link's URL, which the row's own `dropHandlers`-driven `onDrop` would then pass to `onDropScreen` as if it were a screen name — a spurious drop. The chevron's existing `onClick={(e) => { e.stopPropagation(); onToggleExpand(node.path) }}` must be updated to also call `e.preventDefault()` — `onClick={(e) => { e.preventDefault(); e.stopPropagation(); onToggleExpand(node.path) }}` — since it's now nested inside the `AppLink`: `stopPropagation()` alone stops the click from reaching the `AppLink`'s own listener but does not stop the anchor's native navigation default action, which depends solely on `preventDefault()` (see Design). Inside the `AppLink`, render the name conditionally instead of today's renaming/non-renaming branch (lines 305-309):
     `{renamingPath !== node.path && <span className="truncate">{node.name}</span>}`
     (the `flex-1 min-w-0` that mattered for layout now lives on the `AppLink` itself — see above; the
     span no longer needs to carry it). While renaming, the name `<span>` is simply omitted, not
     swapped for the input.
   - When `renamingPath === node.path`, render `renameInput(node)` as a **sibling immediately after
     the `AppLink`** (not inside it) — this is the fix for the `<input>`-nested-in-`<a>` violation,
     the same class of problem the plan already avoids for the "new subfolder"/"folder actions"
     buttons (a focusable `<input>` is disallowed interactive content inside an `<a>`, same as
     `<button>`). `renameInput`'s own className already includes `flex-1 min-w-0` (line 196), so as a
     sibling it naturally takes over the flex slot the name `<span>` would otherwise occupy in the
     row — no layout change needed to `renameInput` itself.
   - The "new subfolder" `<button>` (line 311) and the folder-actions `<button>`+menu (lines 322-356)
     stay as siblings *after* the `AppLink` (and after the conditional `renameInput`), structurally
     outside it — same pattern `ScreensPage.tsx` already uses to keep real `<button>`s out of an `<a>`.
   - Root Home row (line 371): same treatment as `renderScreen` — no nested buttons, so replace the
     `<div role="button" ...>` itself with `<AppLink href={folderHref('')} replace={folderNavReplace} onClick={() => onSelectFolder('')} draggable={false} {...dropHandlers('')} className={...}>` wrapping the `Home` icon and label. `draggable={false}` is needed for the same reason as the folder row: the Home row keeps `dropHandlers('')` as a drop target, and without it the native anchor drag would leak the Home URL into `dataTransfer`'s `text/plain`, tripping `onDropScreen`.
   - `renameInput`'s existing `onClick={(e) => e.stopPropagation()}` (line 187) is no longer
     load-bearing once the input is a sibling outside the `AppLink` — a click on the input can no
     longer reach the `AppLink`'s `onClick` regardless. Harmless to leave in place; don't rely on it
     as the mechanism preventing navigation.

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
     - once a folder is put into rename mode (`renamingPath` set to that folder's path — either by
       driving the "Rename" menu action or by asserting on a component rendered directly with that
       state), the rename `<input>` is *not* inside the folder's `<a>` either (same
       `link.contains(input)` assertion), guarding the `<input>`-nested-in-`<a>` fix.
     - a Ctrl-click (and separately a Cmd/meta-click) on a folder's and a screen's `<a>` still calls
       `onSelectFolder`/`onSelectScreen` (the search-clearing side effect) but does **not** call
       `navigate()` or change the current tab's location — e.g. `fireEvent.click(link, { ctrlKey:
       true })` inside a `MemoryRouter`, then assert the router's location/history is unchanged (or
       that a mocked `navigate` was not called), guarding against the double-navigation/Ctrl-click
       regression. Also assert a plain, unmodified click results in exactly one navigation (no
       duplicate history entry).

## Files to Modify

- `analytics-web-app/src/components/FolderTree.tsx`
- `analytics-web-app/src/components/layout/Sidebar.tsx`
- `analytics-web-app/src/components/__tests__/FolderTree.test.tsx`

## Trade-offs

- **`onClick`'s search-clearing side effect still runs on a modified click (new tab).** With
  `onSelectFolder`/`onSelectScreen` made navigation-free (see Design), opening a folder/screen in a
  new tab via Ctrl/Cmd/middle-click still clears the search box in the *current* tab, since we never
  call `preventDefault()` and the callback always runs. This is now a genuinely minor, purely
  cosmetic side effect — no navigation happens in the current tab, only the search box empties.
  Fixing even this remainder would require detecting modifier/button state in `onClick` and skipping
  the side effect, which adds complexity disproportionate to a one-line UX nit. Left as-is; flagged
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
