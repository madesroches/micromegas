# Sidebar Collapses on Screen Search Plan

## Overview
[Issue #1439](https://github.com/madesroches/micromegas/issues/1439): typing the first character into the sidebar's "Search screens & folders" box collapses the flyout panel back to the narrow icon rail, right when the user starts typing. Root cause is a routing structure bug, not a state bug in the search box itself: `Sidebar` is mounted fresh inside every route's own `PageLayout` instead of once at the app-shell level, so navigating between two different routes fully unmounts and remounts it, resetting its `isOpen` (flyout expanded/collapsed) state to its `false` default. The fix makes `Sidebar` a genuinely persistent, single-instance component that survives route navigation, which a code comment in `ScreensPage.tsx` already assumes is true.

## Current State

### Why the collapse happens
`Sidebar.tsx`'s flyout is controlled purely by hover/blur-driven local state:

```tsx
// analytics-web-app/src/components/layout/Sidebar.tsx:63
const [isOpen, setIsOpen] = useState(false)
...
// :285-289
onMouseEnter={() => setIsOpen(true)}
onMouseLeave={closeUnlessFocusInside}
onBlurCapture={(e) => { if (!asideRef.current?.contains(e.relatedTarget as Node)) setIsOpen(false) }}
```

The search box's `onChange` handler is `updateSearch` (`Sidebar.tsx:188-200`):

```tsx
const updateSearch = useCallback((value: string) => {
  setSearchQuery(value)
  if (!isScreensPage && !value) return
  const next = new URLSearchParams(isScreensPage ? searchParams : undefined)
  if (value) next.set('q', value)
  else next.delete('q')
  const url = `/screens?${next.toString()}`
  if (isScreensPage) navigate(url, { replace: true })
  else navigate(url) // push navigation when not already on /screens
}, [isScreensPage, searchParams, navigate])
```

- If the user is already on `/screens`, this is a same-route `replace` navigation — the mounted `ScreensPage`/`PageLayout`/`Sidebar` instance is untouched, `isOpen` survives.
- If the user is on any **other** page (e.g. `/processes`, `/screen/:name`) and types the first character, this is a **push navigation to a different route** (`/screens?q=<char>`). React Router unmounts the current route's element and mounts `ScreensPage`'s element instead.

`Sidebar` is rendered inside `PageLayout` (`analytics-web-app/src/components/layout/PageLayout.tsx:39`), and **every** route component builds its own `<PageLayout>` — there is no shared layout wrapping `<Routes>` (`analytics-web-app/src/router.tsx:31-54` is a flat list of `<Route path=... element={<Page/>}>`, no nested layout route). So the route change above unmounts the old page's `PageLayout`/`Sidebar` and mounts a brand-new `Sidebar` instance on `ScreensPage`, which re-initializes `isOpen` to `false` — the flyout visually collapses. Because no real mouse movement occurs, the flyout doesn't reopen until the user moves the cursor again.

After that first keystroke the user is on `/screens`, so every subsequent character uses the `replace` branch on the same route — no more remounts, no more collapsing. This exactly matches "collapses after the first character."

Confirming this is unintentional: `ScreensPage.tsx:48` already has a comment describing `Sidebar` as **"the persistent sidebar (Sidebar.tsx)"** — the code's own intent is a single, app-shell-level sidebar instance, but the current routing setup doesn't deliver that.

### Supporting structure
- `analytics-web-app/src/router.tsx:31-54` — flat `<Routes>` list; `/login` and app pages (`/processes`, `/screens`, `/screen/:name`, `/admin/*`, etc.) are siblings with no shared layout route.
- `analytics-web-app/src/components/layout/PageLayout.tsx:34-45` — `PageLayoutContent` renders `<div className="h-screen ... flex-col"><Header/><div className="flex flex-1 min-h-0"><Sidebar/><main>{children}</main>{rightPanel}</div></div>`. Every one of the ~15 route files wraps its content in its own `<PageLayout>` (see `ProcessesPage.tsx`, `ScreensPage.tsx`, `ScreenPage.tsx`, `AdminPage.tsx`, etc.), each passing different `onRefresh`/`rightPanel`/`timeRangeControl`/`processId` props.
- `analytics-web-app/src/routes/LoginPage.tsx` and `NotFoundPage.tsx` do **not** use `PageLayout`/`Sidebar` at all — any fix must keep those two sidebar-free.
- Route-level tests (`PerformanceAnalysisPage.test.tsx`, `ProcessMetricsPage.test.tsx`, `MapsPage.test.tsx`) already mock `@/components/layout`'s `PageLayout` as a pass-through wrapper (`MapsPage.test.tsx:34-36`), so they render pages standalone via `MemoryRouter` without going through `router.tsx`/`AppRouter` at all — they won't be affected by moving `Sidebar` out of `PageLayout`.

## Design

Make `Sidebar` a true app-shell element, mounted once above `<Routes>`, instead of something every page re-creates. Use React Router's standard pathless layout-route pattern so app pages keep their existing paths and prop signatures untouched, while `/login` and the 404 page remain sidebar-free.

```
Before:                              After:

<Routes>                             <Routes>
  <Route path="/login" .../>           <Route path="/login" .../>
  <Route path="/processes"             <Route element={<AppShell/>}>  <- renders Sidebar + <Outlet/>
    element={<ProcessesPage/>}/>         <Route path="/processes"
  <Route path="/screens"                   element={<ProcessesPage/>}/>
    element={<ScreensPage/>}/>            <Route path="/screens"
  ...                                        element={<ScreensPage/>}/>
  <Route path="*" .../>                  ...
</Routes>                                </Route>
                                        <Route path="*" .../>
each page's <PageLayout>             </Routes>
  independently renders <Sidebar/>   each page's <PageLayout> only renders
                                        Header + main + rightPanel (no Sidebar)
```

`AppShell` owns the height/flex chrome that `PageLayout` used to own for the sidebar row:

```tsx
// analytics-web-app/src/components/layout/AppShell.tsx (new)
import { Outlet } from 'react-router'
import { Sidebar } from './Sidebar'

export function AppShell() {
  return (
    <div className="h-screen bg-app-bg flex">
      <Sidebar />
      <div className="flex-1 flex flex-col min-h-0">
        <Outlet />
      </div>
    </div>
  )
}
```

`PageLayout.tsx` drops `Sidebar` and the outer `h-screen` wrapper (that's now `AppShell`'s job):

```tsx
function PageLayoutContent({ children, onRefresh, rightPanel, ... }: PageLayoutProps) {
  return (
    <div className="h-full flex-1 min-w-0 text-theme-text-primary flex flex-col">
      <Header .../>
      <div className="flex flex-1 min-h-0">
        <main className="flex-1 overflow-auto flex flex-col">{children}</main>
        {rightPanel}
      </div>
    </div>
  )
}
```

Since `AppShell` is a pathless layout route (`<Route element={<AppShell/>}>` with no `path`), the nested routes' paths are unaffected — no per-page file needs its `path` changed, and none of the `<PageLayout ...props>` call sites need to change either, since `PageLayout`'s public props are unchanged.

## Implementation Steps

1. **Create `analytics-web-app/src/components/layout/AppShell.tsx`** — renders `Sidebar` once plus an `<Outlet/>` for the active route, per the snippet above.
2. **Edit `analytics-web-app/src/components/layout/PageLayout.tsx`** — remove the `Sidebar` import and `<Sidebar />` render; remove the outer `h-screen` wrapper div in `PageLayoutContent` (replace with a flex-child-friendly wrapper, since height/background now come from `AppShell`).
3. **Edit `analytics-web-app/src/components/layout/index.ts`** — add `export { AppShell } from './AppShell'`.
4. **Edit `analytics-web-app/src/router.tsx`** — wrap all routes except `/login` and `*` (`NotFoundPage`) in a pathless `<Route element={<AppShell />}>` parent, per the diagram above. Import `AppShell` from `@/components/layout`.
5. **Manual verification** (see Testing Strategy) — confirm the flyout stays open through the first keystroke of a sidebar search initiated from a non-`/screens` page, and that `/login` / 404 still render without a sidebar.

## Files to Modify
- `analytics-web-app/src/components/layout/AppShell.tsx` (new)
- `analytics-web-app/src/components/layout/PageLayout.tsx`
- `analytics-web-app/src/components/layout/index.ts`
- `analytics-web-app/src/router.tsx`

## Trade-offs
- **Considered:** patch `Sidebar.tsx` locally (e.g. stash `isOpen`/`expandedPaths` in a module-level variable that survives remounts). Rejected — it's a workaround for a remount that shouldn't be happening at all, doesn't fix the underlying duplicated-per-page layout, and leaves the same remount hazard for any future sidebar state (the code comment already claims persistence that doesn't actually hold).
- **Considered:** full `Outlet`-context refactor where `Header`'s per-page config (`rightPanel`, `timeRangeControl`, `processId`, refresh interval, etc.) is also lifted to a shared layout route. Rejected as out of scope — this issue is specifically about the sidebar, `Header` remounting per navigation isn't reported as broken, and lifting `Header` too would touch all ~15 route files' prop wiring for no benefit to this bug.
- **Side effect (net positive):** since `Sidebar` now mounts once instead of on every navigation, `loadFolders()` (screens/folders fetch) and the `useFoldersChangedListener` subscription only run once per session instead of once per route change, and tree `expandedPaths` / scroll state persist across navigation — matching the "persistent sidebar" intent already stated in `ScreensPage.tsx`'s comment.

## Testing Strategy
- `yarn lint` / `yarn type-check` / `yarn test` in `analytics-web-app/` — existing route tests mock `PageLayout` as a pass-through and don't exercise `router.tsx`, so they should be unaffected; run them to confirm.
- Manual verification in the browser (`yarn dev`):
  1. Load `/processes` (or any non-`/screens` page), hover the sidebar to open the flyout, click into the search box, and type a character — confirm the flyout stays open (no collapse) and the app navigates to `/screens?q=<char>` with matching results shown.
  2. Confirm `/login` renders without a sidebar.
  3. Confirm the 404 page (an unmatched path) renders without a sidebar.
  4. Navigate between a few different app pages and confirm the sidebar's expanded folder state and open/closed flyout state persist sensibly rather than resetting each time.

## Open Questions
- None — root cause and fix are both confirmed against the current code.
