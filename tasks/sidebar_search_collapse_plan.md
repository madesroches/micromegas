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

`AppShell` owns the `h-screen` root that `Sidebar` is now positioned against. It must not render `Sidebar` any earlier than each page's own `AuthGuard` (`analytics-web-app/src/components/AuthGuard.tsx`) would have let that page's content through — otherwise `Sidebar`'s mount effect (`Sidebar.tsx:104-109`, which unconditionally calls `loadFolders()`) fires during loading/unauthenticated/error/admin-denied states it previously never reached. `AppShell` reads the same `useAuth()` status `AuthGuard` uses, and — since every current admin-only route lives under the `/admin` prefix (`AdminPage`, `DataSourcesPage`, `ExportScreensPage`, `ImportScreensPage`, `MapsPage`, all rendering `<AuthGuard requireAdmin>`) — approximates each page's `requireAdmin` check with a path prefix test, without needing to plumb per-route auth requirements through the router. `Sidebar` itself is rendered out of flow (`fixed`), not as a flex sibling of the page content, so it doesn't touch `PageLayout`'s own layout at all:

```tsx
// analytics-web-app/src/components/layout/AppShell.tsx (new)
import { Suspense } from 'react'
import { Outlet, useLocation } from 'react-router'
import { useAuth } from '@/lib/auth'
import { Sidebar } from './Sidebar'

function OutletFallback() {
  return <div className="h-screen flex items-center justify-center text-theme-text-secondary">Loading...</div>
}

export function AppShell() {
  const { status, user } = useAuth()
  const { pathname } = useLocation()
  const isAdminRoute = pathname.startsWith('/admin')
  const showSidebar = status === 'authenticated' && (!isAdminRoute || user?.is_admin)

  return (
    <div className="h-screen bg-app-bg relative">
      {showSidebar && (
        <div className="fixed top-16 left-0 bottom-0 hidden sm:flex">
          <Sidebar />
        </div>
      )}
      <Suspense fallback={<OutletFallback />}>
        <Outlet />
      </Suspense>
    </div>
  )
}
```

The nested `<Suspense>` around `<Outlet/>` is **not** required to keep `AppShell`/`Sidebar` mounted: this app uses a plain `<BrowserRouter>` (`main.tsx:46`, react-router 8.3.0), whose location updates go through `React.startTransition` by default, and under React 19 a transition update that suspends does not swap already-visible content for an ancestor `<Suspense>`'s fallback — React just holds the current UI until the lazy chunk resolves. So navigating to a not-yet-loaded route chunk (e.g. `/screens`) never unmounts `AppShell` via the top-level `<Suspense fallback={<PageLoader/>}>` in `router.tsx` (`router.tsx:33-53`), and this inner `<Suspense>` plays no part in fixing the bug this plan is about. It's kept anyway purely as a scoped loading fallback — it confines any `Loading...` UI to the `<Outlet/>` region (e.g. on the very first render before a transition is in flight) rather than replacing the whole page via the top-level boundary — which is a reasonable, but optional, defensive choice. The top-level `<Suspense>` in `router.tsx` stays as-is regardless; it's still needed for `/login`, `NotFoundPage`, and first paint before `AppShell` has mounted.

`PageLayout.tsx` drops the `Sidebar` render (moved to `AppShell`) and pads its content row with `sm:pl-14` to leave room for the now out-of-flow sidebar rail. It keeps its own `h-screen bg-app-bg` wrapper and `Header` exactly as today, so `Header` still spans the full window width above the content row — the visual layout is unchanged from what's shipped today:

```tsx
function PageLayoutContent({ children, onRefresh, rightPanel, ... }: PageLayoutProps) {
  return (
    <div className="h-screen bg-app-bg text-theme-text-primary flex flex-col">
      <Header .../>
      <div className="flex flex-1 min-h-0 sm:pl-14">
        <main className="flex-1 overflow-auto flex flex-col">{children}</main>
        {rightPanel}
      </div>
    </div>
  )
}
```

Because `Sidebar` is positioned `fixed` (not a flex sibling of the content column), it renders below `Header` and to the left of the content by construction — there's no rail spanning the full viewport height and no flyout/`Header` overlap to separately fix.

Since `AppShell` is a pathless layout route (`<Route element={<AppShell/>}>` with no `path`), the nested routes' paths are unaffected — no per-page file needs its `path` changed, and none of the `<PageLayout ...props>` call sites need to change either, since `PageLayout`'s public props are unchanged.

## Implementation Steps

1. **Create `analytics-web-app/src/components/layout/AppShell.tsx`** — renders `Sidebar` once, gated on `useAuth()` status (plus an `/admin`-prefix check standing in for `requireAdmin`) so it doesn't mount during loading/unauthenticated/error/admin-denied states, wrapped in a `fixed top-16 left-0 bottom-0 hidden sm:flex` container so it renders out of flow instead of as a layout sibling of the page content, and wraps `<Outlet/>` in its own `<Suspense>` as a scoped loading fallback for the outlet region (not required to prevent a shell unmount — see Design) — per the snippet above.
2. **Edit `analytics-web-app/src/components/layout/Header.tsx`** to give `<header>` a fixed, known height matching its actual rendered size — `Header`'s tallest control is the user-menu button (`px-3 py-1.5` around a `w-7 h-7` avatar = 40px), so with `py-3` (24px) padding plus the 1px `border-b`, and `box-sizing: border-box`, the box is 65px today; drop `py-3` and use `h-16` instead, giving a fixed border-box height of 64px (1px shorter than today, but that's the height the fixed sidebar's `top-16` offset in `AppShell` (Step 1) is written against). Keep the two values in sync (a code comment referencing the other file is enough — no need for a CSS variable) — the 40px tallest-control measurement is why `Header`'s height can't be shrunk without either clipping that control or opening a gap between `Header` and the fixed `Sidebar` below it.
3. **Edit `analytics-web-app/src/components/layout/PageLayout.tsx`** — remove the `Sidebar` import and `<Sidebar />` render, and add `sm:pl-14` to the content row div (`<div className="flex flex-1 min-h-0">`, `PageLayout.tsx:38`) so page content still starts to the right of where the 56px-wide (`w-14`) sidebar rail visually sits, now that `Sidebar` is rendered out of flow in `AppShell` instead of in-flow here. Leave `PageLayoutContent`'s outer `h-screen bg-app-bg` wrapper and `Header` untouched.
4. **Edit `analytics-web-app/src/components/layout/index.ts`** — add `export { AppShell } from './AppShell'`.
5. **Edit `analytics-web-app/src/router.tsx`** — wrap all routes except `/login` and `*` (`NotFoundPage`) in a pathless `<Route element={<AppShell />}>` parent, per the diagram above. Import `AppShell` from `@/components/layout`. Leave the existing top-level `<Suspense fallback={<PageLoader/>}>` wrapping the whole `<Routes>` tree unchanged — it still covers `/login`, `NotFoundPage`, and first paint; because navigation runs as a `React.startTransition` (see Design), it also won't unmount `AppShell`/`Sidebar` when a later route's lazy chunk suspends. `AppShell`'s own internal `<Suspense>` (step 1) plays no part in that — it's only a scoped fallback for the `<Outlet/>` region.
6. **Edit `analytics-web-app/src/routes/ImportScreensPage.tsx`** — add `import { notifyFoldersChanged } from '@/lib/folders-sync'`, and call `notifyFoldersChanged()` once, after the `for` loop over `selectedEntries` that calls `importScreen()` (`ImportScreensPage.tsx:197-211`) has finished, conditional on at least one result that isn't `skipped`/`error`. Don't call it inside the loop — `importScreen()` runs once per selected screen, and `notifyFoldersChanged()` triggers `Sidebar`'s listener to refetch folders/screens (`Sidebar.tsx:93-102,111`), so a per-iteration call would fire that refetch once per imported screen instead of once for the whole import. This gap already exists today but is masked because `Sidebar` currently remounts and refetches on every navigation; once `Sidebar` persists across navigation (this plan), an import would otherwise leave the sidebar's screen/folder tree stale until a full page reload.
7. **Create `analytics-web-app/src/components/layout/__tests__/AppShell.test.tsx`** — the new regression test described in Testing Strategy: render the real route tree from `router.tsx` under a `MemoryRouter`, with lazy pages and `Sidebar` mocked, navigate from `/processes` to `/screens`, and assert the `Sidebar` mock's mount counter is still 1 (not remounted). Also add two gate cases covering the auth/admin check itself: mock `useAuth` to return `status: 'loading'` (and separately `status: 'unauthenticated'`) and assert the `Sidebar` mock never mounts at `/processes`; and render at `/admin/maps` with `useAuth` returning `status: 'authenticated', user: { is_admin: false }` and assert the same. These catch a mis-written gate (e.g. `status !== 'unauthenticated'`), whose failure mode is `Sidebar`'s mount effect firing unauthenticated `loadFolders()`/`listScreens()` calls on every page load.
8. **Manual verification** (see Testing Strategy) — confirm the flyout stays open through the first keystroke of a sidebar search initiated from a non-`/screens` page, that `/login` / 404 still render without a sidebar, and that the open flyout never covers `Header`'s logo or controls.

## Files to Modify
- `analytics-web-app/src/components/layout/AppShell.tsx` (new)
- `analytics-web-app/src/components/layout/Header.tsx`
- `analytics-web-app/src/components/layout/PageLayout.tsx`
- `analytics-web-app/src/components/layout/index.ts`
- `analytics-web-app/src/router.tsx`
- `analytics-web-app/src/routes/ImportScreensPage.tsx`
- `analytics-web-app/src/components/layout/__tests__/AppShell.test.tsx` (new)

## Trade-offs
- **Considered:** patch `Sidebar.tsx` locally (e.g. stash `isOpen`/`expandedPaths` in a module-level variable that survives remounts). Rejected — it's a workaround for a remount that shouldn't be happening at all, doesn't fix the underlying duplicated-per-page layout, and leaves the same remount hazard for any future sidebar state (the code comment already claims persistence that doesn't actually hold).
- **Considered:** full `Outlet`-context refactor where `Header`'s per-page config (`rightPanel`, `timeRangeControl`, `processId`, refresh interval, etc.) is also lifted to a shared layout route. Rejected as out of scope — this issue is specifically about the sidebar, `Header` remounting per navigation isn't reported as broken, and lifting `Header` too would touch all ~15 route files' prop wiring for no benefit to this bug.
- **Considered:** render `Sidebar` in-flow as a flex sibling of the content column inside `AppShell`, so it spans the full viewport height as a rail flush with the top of the window. Rejected — it changes the visual layout of every page (`Header` no longer spans the full window width) and introduces a flyout/`Header` overlap hazard that then has to be separately fixed, for no benefit over the out-of-flow approach actually used (`Sidebar` positioned `fixed`, `PageLayout`'s content row padded with `sm:pl-14`), which keeps today's layout pixel-for-pixel and still moves `Sidebar` to `AppShell` at the same cost.
- **Side effect (net positive):** since `Sidebar` now mounts once instead of on every navigation, `loadFolders()` (screens/folders fetch) and the `useFoldersChangedListener` subscription only run once per session instead of once per route change, and tree `expandedPaths` / scroll state persist across navigation — matching the "persistent sidebar" intent already stated in `ScreensPage.tsx`'s comment. This does surface a pre-existing gap: `ImportScreensPage.tsx` never calls `notifyFoldersChanged()` after a successful import, which today is masked by `Sidebar` remounting (and refetching) on every navigation but would otherwise leave the sidebar's tree stale after an import once `Sidebar` persists — Implementation Step 6 closes that gap as part of this change, with a single call after the import loop rather than one per imported screen.
- **Accepted:** fixing `Header`'s height (Implementation Step 2) means `Header` can no longer grow taller to fit content (e.g. wrapped text) without the fixed sidebar's `top-16` offset (Implementation Step 1) drifting out of sync. Given `Header`'s current content is a single row of fixed-size controls, this is not expected to happen in practice.

## Testing Strategy
- `yarn lint` / `yarn type-check` / `yarn test` in `analytics-web-app/` — existing route tests mock `PageLayout` as a pass-through and don't exercise `router.tsx`, so they should be unaffected; run them to confirm.
- **New regression test** — `analytics-web-app/src/components/layout/__tests__/AppShell.test.tsx` (or `router.test.tsx`): render the real route tree from `analytics-web-app/src/router.tsx` — not a standalone `AppShell` with dummy routes — under a `MemoryRouter` starting at `/processes`, with the lazy page modules (`ProcessesPage`, `ScreensPage`, etc.) mocked to lightweight stubs. Mock `@/lib/auth`'s `useAuth` to return `{ status: 'authenticated', user: { is_admin: true } }` so `AppShell`'s `showSidebar` gate passes, and mock `Sidebar` (or the `folders-api`/`screens-api` calls it makes on mount, per `Sidebar.tsx:93-109`) so it doesn't need a real backend. Navigate from `/processes` to `/screens` (e.g. via a rendered `<Link>` or `history.push`) and assert the `Sidebar` mock's mount marker (a module-level mount counter) mounted exactly once and is still 1 after the navigation, rather than asserting on visual flyout CSS. Because this exercises the actual router tree, it directly covers the bug mechanism (issue #1439: route change unmounting `Sidebar` and resetting its state) and it would fail on the old flat-`<Routes>` structure, where there is no `AppShell` and the stubbed pages never mount a `Sidebar` at all, so the counter would read 0 after the navigation instead of 1. Note this test only covers the router-restructuring half of the fix: since the lightweight page stubs don't render the real `PageLayout`, it would *not* catch a regression where `Sidebar` was reintroduced into `PageLayout` on top of the new router structure — that would need a separate check (e.g. a lint/grep or code-review rule that `PageLayout.tsx` never imports `Sidebar`) if that failure mode is worth guarding against.
- **New regression test, gate cases** — in the same file, add cases exercising `AppShell`'s auth/admin gate directly, since the mount-counter case above only ever mocks `useAuth` to `{ status: 'authenticated', user: { is_admin: true } }` and so never touches the gate logic: (1) mock `useAuth` to return `status: 'loading'` (and separately `status: 'unauthenticated'`), render at `/processes`, and assert the `Sidebar` mock never mounts; (2) mock `useAuth` to return `status: 'authenticated', user: { is_admin: false }`, render at `/admin/maps` (a real `requireAdmin` route per `router.tsx`), and assert the `Sidebar` mock never mounts there either. These would catch a mis-written gate (e.g. `status !== 'unauthenticated'` instead of `status === 'authenticated'`) whose failure mode — `Sidebar`'s unconditional mount effect (`Sidebar.tsx:104-109`) firing unauthenticated `loadFolders()`/`listScreens()` calls during loading/unauthenticated/admin-denied states — the manual checklist below (which only covers `/login` and 404, both outside `AppShell` entirely) does not exercise.
- Manual verification in the browser (`yarn dev`):
  1. Load `/processes` (or any non-`/screens` page), hover the sidebar to open the flyout, click into the search box, and type a character — confirm the flyout stays open (no collapse) and the app navigates to `/screens?q=<char>` with matching results shown.
  2. Confirm `/login` renders without a sidebar.
  3. Confirm the 404 page (an unmatched path) renders without a sidebar.
  4. Navigate between a few different app pages and confirm the sidebar's expanded folder state and open/closed flyout state persist sensibly rather than resetting each time.
  5. Confirm the page layout is unchanged from before this change: on any page, `Header` still spans the full window width above the content row, and `Sidebar` still sits in its usual place below `Header` rather than as a full-height rail — including on a page with visible header controls (e.g. `/processes`), where hovering the sidebar open should show the flyout rendering below `Header` without covering the logo, time-range picker, or pivot button.

## Open Questions
- None — root cause and fix are both confirmed against the current code.
