# Notebook Tab Execution State (Favicon/Title) Plan

## Overview

Issue [#1443](https://github.com/madesroches/micromegas/issues/1443): when a notebook cell
(or a whole notebook load) runs for a while, the user tabs away and has no way to tell —
from outside the tab — whether it's still running, finished, or failed. This plan makes the
browser tab itself reflect that: swap the favicon and prefix `document.title` while a screen
is executing, and show a distinct state if it finished with an error, restoring the normal
tab identity once everything is idle.

The state already exists and is already global to the page — `ScreenPage.tsx` tracks
`isExecuting` today (for the header refresh spinner). This plan reuses that plumbing instead
of adding a notebook-only path, and adds an equivalent `hasError` signal alongside it. Since
`ScreenPage` hosts every screen type (table, log, metrics, process-list, notebook), all of
them get the tab indicator for free — notebooks (the motivating case) included.

## Current State

- **Tab identity**: `analytics-web-app/index.html:6-7` sets a static `<title>Micromegas</title>`
  and `<link rel="icon" ... href="./icon.svg">`. `usePageTitle`
  (`src/hooks/usePageTitle.ts`) is the only place that touches `document.title` at runtime —
  it sets `"{title} - Micromegas"` or falls back to `"Micromegas"`, and is called from every
  route page (`ScreenPage.tsx:63`, plus ~15 others). Nothing currently touches the favicon
  after initial load.
- **Execution state, already page-global**: `ScreenPage.tsx:94` holds
  `const [isExecuting, setIsExecuting] = useState(false)`, fed by each renderer via the
  `onExecutingChange` callback on `ScreenRendererProps` (`src/lib/screen-renderers/index.ts:42`).
  Every renderer already reports this:
  - `NotebookRenderer.tsx:345-349` — `isExecuting = Object.values(cellStates).some(s => s.status === 'loading')`,
    derived from `useCellExecution`'s per-cell `cellStates` (`useCellExecution.ts:103`,
    statuses: `'idle' | 'loading' | 'success' | 'error' | 'blocked'`, see `notebook-types.ts:101,165`).
    This already covers the initial-load case: `useCellExecution` auto-executes all cells on
    mount (`useCellExecution.ts:373-379`), so `isExecuting` goes true immediately.
  - `TableRenderer.tsx:79`, `LogRenderer.tsx:324`, `ProcessListRenderer.tsx:116` —
    `streamQuery.isStreaming`.
  - `MetricsRenderer.tsx:77` — `query.isLoading`.
  `isExecuting` is currently consumed only by `RefreshIntervalPicker` (spinning refresh icon,
  via `PageLayout` → `Header`) and to pause auto-refresh (`useRefreshInterval.ts`).
- **No page-global error signal today.** Each renderer already computes an error string for
  its own display (`TableRenderer.tsx:76` `queryError`, `LogRenderer.tsx:321`, `MetricsRenderer.tsx`
  `query.error`, `ProcessListRenderer.tsx:113`, and for notebooks, any `cellStates[...].status === 'error'`),
  but none of it is reported up to `ScreenPage`.
- **Base path handling**: `vite.config.ts:41` uses `base: './'` and the app can be served
  under a configurable `MICROMEGAS_BASE_PATH` (`main.tsx:46`, `BrowserRouter basename`). The
  favicon `<link>` therefore must not be pointed at a hardcoded absolute path — see Design.

## Design

### 1. Report error state up, same shape as `isExecuting`

Add `onErrorChange?: (hasError: boolean) => void` to `ScreenRendererProps`
(`src/lib/screen-renderers/index.ts`), mirroring `onExecutingChange` exactly. Each renderer
adds one more `useEffect`, reusing the error value it already computes:

| Renderer | Existing error value | New effect |
|---|---|---|
| `NotebookRenderer` | `Object.values(cellStates).some(s => s.status === 'error')` | alongside the existing `isExecuting` memo |
| `TableRenderer` | `queryError` | `useEffect(() => { onErrorChange?.(!!queryError) }, [queryError, onErrorChange])` |
| `LogRenderer` | `queryError` | same pattern |
| `MetricsRenderer` | `query.error` | same pattern |
| `ProcessListRenderer` | `queryError` | same pattern |

`ScreenPage.tsx` adds `const [hasError, setHasError] = useState(false)` next to `isExecuting`
and passes `onErrorChange={setHasError}` alongside the existing `onExecutingChange={setIsExecuting}`.

Precedence when both are possible: busy wins while `isExecuting` is true (a running
re-execution is actively clearing/replacing the old error), error only applies once idle.

### 2. Tab state as three variants: idle / busy / error

```
isExecuting → true                    →  BUSY
isExecuting → false, hasError → true  →  ERROR
isExecuting → false, hasError → false →  IDLE
```

### 3. Favicon assets

Add two sibling SVGs next to `public/icon.svg`, each a copy of `icon.svg` (same
`viewBox="0 0 32 32"`) with one additional badge overlay in the bottom-right corner, per the
mockup at `tasks/tab_execution_state_mockup.html`:

- `public/icon-busy.svg` — append:
  ```svg
  <circle cx="23" cy="23" r="7" fill="#ffb300" stroke="#ffffff" stroke-width="1.5"/>
  ```
- `public/icon-error.svg` — append the same circle in a different color plus a white
  exclamation glyph, so busy/error are distinguishable by shape, not just color:
  ```svg
  <circle cx="23" cy="23" r="7" fill="#d1382b" stroke="#ffffff" stroke-width="1.5"/>
  <line x1="23" y1="19.4" x2="23" y2="23.5" stroke="#ffffff" stroke-width="1.6" stroke-linecap="round"/>
  <circle cx="23" cy="26.2" r="0.9" fill="#ffffff"/>
  ```

`#d1382b` is a literal, hardcoded in the SVG — not a reference to the app's `--accent-error`
CSS variable (`#f87171` in `src/styles/globals.css`). `icon-error.svg` is loaded by the
browser as a standalone document via `<link rel="icon">`, not inlined into the app's DOM, so
it has no access to the host page's `:root` custom properties; a live link to `--accent-error`
isn't possible here regardless of which red is chosen.

Both are copies of `icon.svg` with the overlay element(s) above — no new build tooling needed,
they're static assets like the existing icon. Kept static (no `<animate>`) for cross-browser
favicon reliability — animated SVG favicons render inconsistently in tab icons across
browsers.

### 4. Runtime favicon/title swap

New hook `src/hooks/useTabExecutionState.ts`:

```ts
export function useTabExecutionState(state: 'idle' | 'busy' | 'error'): void {
  useEffect(() => {
    const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]')
    if (!link) return
    // Capture the idle href once, from the link tag's own resolved URL, so this
    // works regardless of base path / how index.html's relative href resolved.
    const idleHref = link.dataset.idleHref ?? (link.dataset.idleHref = link.href)
    link.href =
      state === 'busy' ? idleHref.replace(/icon\.svg$/, 'icon-busy.svg') :
      state === 'error' ? idleHref.replace(/icon\.svg$/, 'icon-error.svg') :
      idleHref
    // Unmount reset: ScreenPageContent has no persistent layout/Outlet (router.tsx lists
    // `/screen/new` and `/screen/:name` as plain sibling <Route>s), so navigating to any
    // other route (`/processes`, `/screens`, `/admin`, ...) fully unmounts this component
    // without `load()`'s same-component reset (Design §5) ever running. Without this
    // cleanup, a busy/error favicon set here would persist indefinitely on unrelated pages.
    return () => {
      link.href = idleHref
    }
  }, [state])
}
```

Reading `link.href` (not the raw `href` attribute) gives the browser-resolved absolute URL,
so it's correct under any `MICROMEGAS_BASE_PATH` / relative-base setup without this hook
needing to know about base paths at all. `link.dataset.idleHref` caches that resolved URL on
the element itself so repeated toggles don't re-derive it. The cleanup function reverts the
favicon to idle both between re-renders (immediately superseded by the next render's effect,
so no visible flicker) and, more importantly, on unmount — which is the only way the favicon
gets reset when the user navigates away from the screen-route family entirely rather than to
another screen.

Title prefix: extend `usePageTitle` with an optional second argument rather than
introducing a second title-writing hook (avoids two hooks racing to set `document.title`):

```ts
export function usePageTitle(title: string | undefined | null, busy = false): void {
  useEffect(() => {
    const base = title ? `${title} - ${APP_NAME}` : APP_NAME
    document.title = busy ? `[*] ${base}` : base
  }, [title, busy])
}
```

All ~15 existing call sites keep working unchanged (`busy` defaults to `false`). Only
`ScreenPage.tsx` passes `true`. Error state is not reflected in the title prefix — the
favicon badge is the error signal, matching the issue's framing that the title prefix exists
mainly for icon-only pinned tabs, and busy/idle is the transition that most needs a title
cue.

### 5. Wiring in `ScreenPage.tsx`

The busy/error/idle precedence is extracted into a small pure function so it can be unit
tested independently of `ScreenPage`'s rendering (see Testing Strategy):

```ts
export function computeTabState(isExecuting: boolean, hasError: boolean): 'idle' | 'busy' | 'error' {
  return isExecuting ? 'busy' : hasError ? 'error' : 'idle'
}
```

```ts
const [hasError, setHasError] = useState(false)
const tabState = computeTabState(isExecuting, hasError)
useTabExecutionState(tabState)
...
<Renderer ... onExecutingChange={setIsExecuting} onErrorChange={setHasError} />
```

`isExecuting` is declared at `const [isExecuting, setIsExecuting] = useState(false)`, which
today sits well after the existing `usePageTitle(pageTitle)` call. Since `usePageTitle` now
needs `isExecuting` as its second argument, the call must move down to after that
declaration (next to where `hasError`/`tabState`/`useTabExecutionState` are wired) rather than
being edited in place — leaving it at its current spot would reference `isExecuting` before
its declaration and fail to build:

```ts
usePageTitle(pageTitle, isExecuting)
```

`hasError` should reset to `false` whenever a fresh execution starts, which it already does
naturally: the moment a cell/query goes back to `loading`, the corresponding renderer's error
effect recomputes to `false` before (or as) `isExecuting` flips true, and `tabState` favors
`busy` regardless.

**Reset on navigation.** The `load()` effect (triggered on `[isNew, name, typeParam]` change,
i.e. navigating to a different screen) already resets `screen`, `baselineConfig`, etc. via
`setScreen(null)` — but today it does *not* reset `isExecuting`/`hasError`, since those only
change via the `onExecutingChange`/`onErrorChange` callbacks, which only fire once the new
screen's `Renderer` mounts (gated behind `isLoadingScreen`, true for the whole fetch). Without
an explicit reset, a busy/error favicon from the previous screen would persist through the
entire "Loading screen..." window of the next one, even though the title already fell back to
the app name. `load()` must therefore also call `setIsExecuting(false)` and `setHasError(false)`
alongside `setScreen(null)`.

This `load()` reset only handles navigation *within* `ScreenPageContent` (same mounted
component, screen-to-screen). It does not run, and doesn't need to, when the user navigates
away from the screen-route family altogether (`/processes`, `/screens`, `/admin`, ...) — since
`router.tsx` has no persistent layout/`<Outlet>` wrapping the screen routes, that kind of
navigation fully unmounts `ScreenPageContent` instead, which is handled by
`useTabExecutionState`'s own unmount cleanup (Design §4). The two resets are complementary,
each covering the case the other can't.

## Implementation Steps

1. **Assets**: add `public/icon-busy.svg` and `public/icon-error.svg` (copies of `icon.svg`
   plus one badge overlay element each).
2. **Hook**: add `src/hooks/useTabExecutionState.ts` per Design §4; extend
   `src/hooks/usePageTitle.ts` with the `busy` parameter.
3. **Renderer prop**: add `onErrorChange?: (hasError: boolean) => void` to
   `ScreenRendererProps` in `src/lib/screen-renderers/index.ts`.
4. **Renderers**: add the error-reporting `useEffect` to `NotebookRenderer.tsx`,
   `TableRenderer.tsx`, `LogRenderer.tsx`, `MetricsRenderer.tsx`, `ProcessListRenderer.tsx`
   (table above).
5. **ScreenPage**: add `hasError` state, compute `tabState` via `computeTabState`, call
   `useTabExecutionState`, wire `onErrorChange` to the `Renderer`, and reset
   `isExecuting`/`hasError` to `false` at the start of the `load()` effect alongside
   `setScreen(null)`. Move the existing `usePageTitle(pageTitle)` call down to after the
   `isExecuting` declaration and pass it as the `busy` argument (`usePageTitle(pageTitle,
   isExecuting)`) — leaving the call at its current spot would reference `isExecuting` before
   its declaration.
6. **Manual verification**: run a notebook with a slow cell (or a query against a large time
   range), tab away, confirm favicon/title change, tab back after completion/failure and
   confirm both states are visually distinct and revert to idle after opening a fresh screen.

## Files to Modify

- `analytics-web-app/public/icon-busy.svg` (new)
- `analytics-web-app/public/icon-error.svg` (new)
- `analytics-web-app/src/hooks/useTabExecutionState.ts` (new)
- `analytics-web-app/src/hooks/usePageTitle.ts`
- `analytics-web-app/src/lib/screen-renderers/index.ts`
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`
- `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx`
- `analytics-web-app/src/lib/screen-renderers/LogRenderer.tsx`
- `analytics-web-app/src/lib/screen-renderers/MetricsRenderer.tsx`
- `analytics-web-app/src/lib/screen-renderers/ProcessListRenderer.tsx`
- `analytics-web-app/src/routes/ScreenPage.tsx`

## Trade-offs

- **Page-global (`ScreenPage`) vs. notebook-only.** The issue is framed around notebooks,
  but `isExecuting` already lives one level up, generic to all screen types, and every
  renderer already reports into it. Scoping the favicon/title change to `NotebookRenderer`
  alone would mean re-deriving "is anything running" from scratch inside one renderer while
  ignoring the identical signal already flowing through `ScreenPage` — that's the kind of
  duplication the existing code deliberately avoids. Extending it to all screen types is a
  natural side effect, not scope creep: a slow table/log/metrics query left running in a
  background tab is exactly the same problem.
- **Static badge vs. animated favicon.** An animated/pulsing busy icon (closer to a spinner)
  is visually nicer but animated SVG favicons are inconsistently honored across browsers
  (some only render the first frame in the tab). A static badge is the reliable choice; Jupyter's
  own busy indicator is also a static swapped icon, not an animation.
- **Extending `usePageTitle` vs. a separate title-writing hook.** Two hooks both writing
  `document.title` on the same page risk fighting each other depending on effect order. A
  single hook with an extra parameter keeps `document.title` writes centralized to one call
  site's effect, at the (small) cost of every caller having a `busy` parameter it mostly
  ignores.
- **Title reflects busy/idle only, not error.** The issue's own notes call the title prefix
  "optional," useful mainly so a pinned/narrow tab (icon-only) still reads busy at a glance.
  The favicon carries the full three-state signal; keeping the title binary avoids inventing
  a title convention for "error" (`[!]`? `[x]`?) that has no precedent elsewhere in the app.

## Testing Strategy

- New unit tests in `src/hooks/__tests__/useTabExecutionState.test.ts` (pattern after
  `useFadeOnIdle.test.ts`): mount with a `<link rel="icon" href=".../icon.svg">` present in
  jsdom, assert `link.href` updates for `'busy'`/`'error'`/`'idle'`, that it's stable under
  repeated toggles (`idleHref` caching doesn't drift), and that unmounting the hook while in
  `'busy'`/`'error'` reverts `link.href` to `idleHref`.
- Extend `usePageTitle` tests (or add `src/hooks/__tests__/usePageTitle.test.ts` if none
  exist) to cover the `busy` prefix.
- Unit test for `computeTabState` (Design §5): assert `busy` wins when both `isExecuting` and
  `hasError` are true, plus the plain `error`/`idle` cases.
- Manual verification per Implementation Steps §6 — this is a browser-chrome effect
  (`document.title`/favicon) that isn't meaningfully exercised by component-level render
  tests.

## Open Questions

None — both prior questions here (badge artwork, and `hasError` reset-on-navigation behavior)
are resolved in Design §3 and §5 respectively.
