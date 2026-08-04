# Adopt the 5 Deferred React Compiler Lint Rules Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1423

## Overview

`eslint-plugin-react-hooks` v7 (landed in #1422) folded the React Compiler rule family into
`recommended`. Five of those rules fire on `analytics-web-app` and were turned off at
`analytics-web-app/eslint.config.js:20-24` to keep #1422 reviewable (106 findings across 48 files —
documented in `tasks/completed/1255_web_js_major_bumps_plan.md`). This plan re-enables all five —
`react-hooks/purity`, `react-hooks/static-components`, `react-hooks/immutability`,
`react-hooks/set-state-in-effect`, `react-hooks/refs` — fixing the two large mechanically-fixable `refs`
clusters and triaging the remaining 42 sites, some of which end as justified per-site `eslint-disable`s
(as the issue explicitly allows), and removes the five `'off'` overrides plus their explanatory comment
from `eslint.config.js`.

The findings were reproduced directly (not taken on faith from the issue): a scratch config spreading
`eslint.config.js` with the five rules flipped to `'error'`, run via `yarn eslint . -c <scratch> -f json`,
reproduces exactly the issue's numbers — 106 findings across 48 files (`purity` 1, `static-components` 8,
`immutability` 4, `set-state-in-effect` 33, `refs` 60) — confirming the issue's inventory is still accurate
against current `main`. The fix patterns below were verified the same way: each candidate refactor shape
was linted in isolation against the same scratch config before being adopted as the plan's recommended
fix, so the patterns in Design are confirmed to satisfy the rule, not assumed.

Ordered as five phases, one per rule, matching the issue's own suggested order (easiest/highest-signal
first) and its acceptance criteria. Four of the five phases are a single commit each; `refs` — by far the
largest, spanning a mechanical ref-write pattern, one behavioral refactor, and an open-ended 42-site
triage — splits into three commits of its own (Pattern 4 moves, the `useNotebookVariables` refactor, then
the long tail plus the rule flip), for seven commits total (eight counting the optional changelog commit).
Each commit is gated green on `python3 build/analytics_web_ci.py`, so the branch bisects cleanly and each
commit's diff stays reviewable.

## Current State

`analytics-web-app/eslint.config.js:14-29`:

```js
rules: {
  ...reactHooks.configs.recommended.rules,
  // React Compiler rules that fire on this codebase (106 findings across 48 files) —
  // turned off here; adoption is deferred and not yet tracked by an issue
  // (see tasks/completed/1255_web_js_major_bumps_plan.md).
  'react-hooks/refs': 'off',
  'react-hooks/set-state-in-effect': 'off',
  'react-hooks/static-components': 'off',
  'react-hooks/immutability': 'off',
  'react-hooks/purity': 'off',
  '@typescript-eslint/no-unused-vars': [...],
  'react-refresh/only-export-components': [...],
},
```

All five default to `'error'` in `eslint-plugin-react-hooks@7.1.1`'s `recommended` config (confirmed in
`tasks/completed/1255_web_js_major_bumps_plan.md`'s Design section), so removing an `'off'` line restores
`'error'` — no explicit severity needs to be written back in.

### Reproduced findings (file:line inventory)

#### `react-hooks/purity` — 1 finding, 1 file

- `src/routes/PerformanceAnalysisPage.tsx:69`

#### `react-hooks/static-components` — 8 findings, 3 files

- `src/routes/ProcessesPage.tsx:339,343,344,347,350,353` (6 usages of one inline-declared component)
- `src/lib/screen-renderers/cells/HgChildPane.tsx:232`
- `src/routes/ScreenPage.tsx:459`

#### `react-hooks/immutability` — 4 findings, 3 files

- `src/components/map/modes/PerspectiveCameraController.tsx:164,169`
- `src/lib/auth.tsx:62`
- `src/lib/screen-renderers/cells/FlameGraphCell.tsx:189`

#### `react-hooks/set-state-in-effect` — 33 findings, 25 files

```
src/components/layout/Sidebar.tsx:105,113,126,137
src/routes/ProcessPage.tsx:157,178,198
src/lib/screen-renderers/MetricsRenderer.tsx:82,89
src/lib/screen-renderers/cells/ImageCell.tsx:43,55
src/routes/ProcessMetricsPage.tsx:227,268
src/components/CellEditor.tsx:52
src/components/XYChart.tsx:594
src/components/layout/TimeRangePicker/CustomRange.tsx:36
src/components/layout/TimeRangePicker/RecentRanges.tsx:10
src/hooks/useDataSourceState.ts:21
src/hooks/useFadeOnIdle.ts:17
src/hooks/useMetricsData.ts:106
src/lib/screen-renderers/LogRenderer.tsx:329
src/lib/screen-renderers/NotebookRenderer.tsx:165
src/lib/screen-renderers/cells/HorizontalGroupCell.tsx:296
src/lib/screen-renderers/cells/LogCell.tsx:207
src/lib/screen-renderers/cells/PerfettoExportCell.tsx:53
src/lib/screen-renderers/cells/TableCell.tsx:48
src/lib/screen-renderers/pagination.tsx:50
src/routes/DataSourcesPage.tsx:65
src/routes/ExportScreensPage.tsx:44
src/routes/ImportScreensPage.tsx:98
src/routes/MapsPage.tsx:67
src/routes/ProcessLogPage.tsx:273
src/routes/ScreensPage.tsx:77
```

#### `react-hooks/refs` — 60 findings, 26 files

```
src/lib/screen-renderers/useNotebookVariables.ts:122,123,135,136,149,154,157,158        (11, dup lines listed once)
src/components/map/hooks/useMapOrbitController.ts:81,83,85,87,89                        (5)
src/lib/screen-renderers/NotebookRenderer.tsx:107,119,731,783,798                        (5)
src/components/XYChart.tsx:187,189,296                                                  (3)
src/lib/screen-renderers/cells/MapCell.tsx:287,575,576                                  (3)
src/lib/screen-renderers/useScreenQuery.ts:68,138                                       (2, 3 findings)
src/routes/ProcessPage.tsx:216,218,220                                                  (3)
src/components/map/MapInstancedMarkers.tsx:68,106                                       (2)
src/lib/screen-renderers/LogRenderer.tsx:277,350                                        (2)
src/lib/screen-renderers/useNotebookAutoRun.ts:40,44                                    (2)
src/lib/screen-renderers/warning-reporter.tsx:52,53                                     (2)
src/routes/ProcessLogPage.tsx:284,287                                                   (2)
src/routes/ProcessMetricsPage.tsx:277,292                                               (2)
src/routes/ProcessesPage.tsx:130,134                                                    (2)
src/routes/perf-analysis/PerformanceMetricsChart.tsx:212,220                            (2)
src/components/map/modes/OrthographicCameraController.tsx:109                          (1)
src/components/map/modes/PerspectiveCameraController.tsx:45                             (1)
src/hooks/useChangeEffect.ts:12                                                         (1)
src/hooks/useRefreshInterval.ts:11                                                      (1)
src/hooks/useScreenConfig.ts:86                                                         (1)
src/lib/screen-renderers/ProcessListRenderer.tsx:120                                    (1)
src/lib/screen-renderers/TableRenderer.tsx:83                                           (1)
src/lib/screen-renderers/cells/HgChildPane.tsx:71                                       (1)
src/lib/screen-renderers/cells/TableCell.tsx:44                                         (1)
src/routes/perf-analysis/MeasureDiscovery.tsx:112                                       (1)
src/routes/perf-analysis/ThreadCoveragePanel.tsx:132                                    (1)
```

(`useScreenQuery.ts` lists 2 distinct lines but 3 findings — one line has two overlapping reports.)

## Design

Five fix patterns cover essentially every finding. Each was verified by linting the pattern in isolation
against a scratch config with all five rules forced to `'error'` (same mechanism as the reproduction
above); none of this is inferred from the rule's prose docs alone.

### Pattern 1 — impure call during render → delegate to the existing pure fallback

`purity`'s one finding computes a fallback time range inline with `Date.now()`:

```tsx
// src/routes/PerformanceAnalysisPage.tsx:69, current
} catch {
  return { label: 'Last 1 hour', from: new Date(Date.now() - 3600000), to: new Date() }
}
```

The component already has a working impure-free path for exactly this fallback:
`parseTimeRange('now-1h', 'now')` (used one `useMemo` up, for `apiTimeRange`) produces the identical
`{ label: 'Last 1 hour', from: <1h ago>, to: <now> }` shape by resolving `Date.now()` inside
`src/lib/time-range.ts` instead of in the component body — the rule only inspects the render function it's
checking, not functions it calls into, so delegating is a real fix, not a relocation of the same problem.
Fix: `return parseTimeRange('now-1h', 'now')`.

### Pattern 2 — component created during render

Two distinct shapes, both in this codebase, with different fixes:

**2a. Locally-declared component (`ProcessesPage.tsx`).** `SortHeader` is declared inside the page
component (`src/routes/ProcessesPage.tsx:225-253`), closing over `handleSort`, `sortField`, and
`sortDirection`, then used 6 times as a table header (`:339,343,344,347,350,353`). Fix: hoist `SortHeader`
to module scope, converting the three closed-over values to explicit props (`sortField`, `sortDirection`,
`onSort`), and pass them at each of the 6 call sites.

**2b. Registry-lookup functions whose return type is a component type (`HgChildPane.tsx`,
`ScreenPage.tsx`).** `getCellRenderer(type)` / `getRenderer(type)` both just index a static, module-level
registry (`CELL_TYPE_METADATA[type].renderer` in `cell-registry.ts:217`; `SCREEN_RENDERERS[typeName]` in
`screen-renderers/index.ts:88`) — they return the same component reference on every call, so there is no
actual "component created during render" here. Verified empirically: the rule flags a JSX tag variable
assigned from *any* call expression whose declared return type is a component type, regardless of what
the function body does or whether the result is wrapped in `useMemo` — but does **not** flag a JSX tag
variable assigned via direct member/index access (`REGISTRY[type]`, or a property read off an
already-computed object). So the fix is to reach the registry via member access instead of through the
getter, at the two render call sites only (the getters themselves stay, since they're used elsewhere
outside render):
- `HgChildPane.tsx:80` already computes `meta = getCellTypeMetadata(child.type)` on the line above;
  change `const CellRenderer = getCellRenderer(child.type)` to `const CellRenderer = meta.renderer` — no
  new call needed, `meta.renderer` is the same value `getCellRenderer` returns.
- `ScreenPage.tsx:345`: import `SCREEN_RENDERERS` (already exported from `screen-renderers/index.ts:87`)
  and change `const Renderer = getRenderer(screenType)` to `const Renderer = SCREEN_RENDERERS[screenType]`.

### Pattern 3 — self-referential closures ("accessed before declared")

Two sites, two different resolutions because the recursion serves different purposes:

**3a. `src/lib/auth.tsx:45-85`, `checkAuth`.** `checkAuth` calls itself (`await checkAuth(true)`) inside
its own `useCallback` body, on the token-refresh-then-retry branch. The retry is a single bounded
one-shot (retry once, with `skipRefresh` guarding against a second refresh attempt) — a plain loop
expresses the same logic without any self-reference:

```tsx
const checkAuth = useCallback(async (skipRefresh = false) => {
  let allowRefresh = !skipRefresh
  while (true) {
    try {
      const response = await fetch(`${getAuthBase()}/auth/me`, { credentials: 'include' })
      if (response.ok) {
        const userData = await response.json()
        setUser(userData); setStatus('authenticated'); setError(null)
        return
      }
      if (response.status === 401 && allowRefresh) {
        const refreshed = await refreshTokens()
        if (refreshed) { allowRefresh = false; continue }
      }
      if (response.status === 401) {
        setUser(null); setStatus('unauthenticated'); setError(null)
      } else {
        setUser(null); setStatus('error'); setError(`Server error: ${response.status}`)
      }
      return
    } catch (err) {
      setUser(null); setStatus('error'); setError(err instanceof Error ? err.message : 'Network error')
      return
    }
  }
}, [refreshTokens])
```

This is a behavior-preserving rewrite (same three branches, same retry-once semantics), and reads more
plainly than the recursive version — a genuine simplification, not a lint workaround.

**3b. `src/lib/screen-renderers/cells/FlameGraphCell.tsx:165-190`, `keyTick`.** This one is a real
recursive animation loop (`requestAnimationFrame(keyTick)` inside `keyTick`), which a `while` loop can't
replace — each iteration must yield to the browser's paint cycle. Fix: hold the latest `keyTick` in a ref,
updated via an effect (Pattern 4 below handles exactly this "assign a ref outside render" shape), and have
the RAF callback go through the ref instead of the closure:

```tsx
const keyTickRef = useRef<() => void>(() => {})
const keyTick = useCallback(() => {
  // ...unchanged body...
  requestRender()
  keyAnimRef.current = requestAnimationFrame(() => keyTickRef.current())
}, [index, requestRender])

useEffect(() => {
  keyTickRef.current = keyTick
})
```

### Pattern 4 — ref mutated during render ("latest ref" props pattern) → move the write into an effect

The dominant `refs` shape in this codebase is the "latest ref" idiom for giving an event handler or a
`useFrame`/RAF loop access to a fresh callback/value without rebinding: `const xRef = useRef(x); xRef.current
= x`, written directly in the render body. Verified fix: move the assignment into a deps-less
`useEffect`/`useLayoutEffect` (runs after every render, so the ref is fresh before any other effect or the
next frame callback reads it) — confirmed clean against the rule.

`src/components/map/hooks/useMapOrbitController.ts:80-89` is the largest concentration (5 of the 60
findings, all five "latest ref" writes back-to-back) and collapses into one block:

```tsx
const onWheelRef = useRef(onWheel)
const getPanSpeedRef = useRef(getPanSpeed)
const getFlyMoveSpeedPerFrameRef = useRef(getFlyMoveSpeedPerFrame)
const onRightDragReAnchorRef = useRef(onRightDragReAnchor)
const onFlyZoomRef = useRef(onFlyZoom)

useLayoutEffect(() => {
  onWheelRef.current = onWheel
  getPanSpeedRef.current = getPanSpeed
  getFlyMoveSpeedPerFrameRef.current = getFlyMoveSpeedPerFrame
  onRightDragReAnchorRef.current = onRightDragReAnchor
  onFlyZoomRef.current = onFlyZoom
})
```

`useLayoutEffect` (not `useEffect`) preserves the ordering the file's own comment already documents
("refs are attached during commit — before any effect fires"), since layout effects run before passive
effects and before the DOM-binding effect a few lines below reads `cameraRef.current`. The same shape
applies to `PerspectiveCameraController.tsx:45` (`mapSceneRef.current = mapScene`) and
`OrthographicCameraController.tsx:109`.

### Pattern 5 — ref read during render for previous-value comparison → track previous value in state instead

`src/lib/screen-renderers/useNotebookVariables.ts:122-158` (11 of the 60 `refs` findings — the single
largest cluster) uses `prevCellKeysRef`/`prevSavedDefaultsRef`/`variableValuesRef` to detect what changed
since the last render and recompute derived variable state, all read and written directly in the hook's
body. This is the documented "adjust state when a value changes" shape — React's own recommended
resolution is to track the previous value in **state**, not a ref, and branch on the comparison directly
in the render body:

```tsx
const [prevCellKeys, setPrevCellKeys] = useState(cellKeys)
const [prevSavedDefaults, setPrevSavedDefaults] = useState(savedDefaultsByName)
if (cellKeys !== prevCellKeys || savedDefaultsByName !== prevSavedDefaults) {
  setPrevCellKeys(cellKeys)
  setPrevSavedDefaults(savedDefaultsByName)
  // ...same recompute logic, writing to `variableValues` state instead of `variableValuesRef`...
}
```

Verified clean against the rule (calling `setState` conditionally, guarded, directly in the render body is
explicitly allowed — it is not "in an effect" and not an unconditional render-time mutation). This is the
one site in the whole issue that's a real behavioral refactor rather than a mechanical move, so it gets
its own careful pass with `useNotebookVariables.test.tsx` run before/after (existing coverage — see
Testing Strategy) rather than a one-line change.

`variableValuesRef` itself is not removed: it's a documented member of `UseNotebookVariablesResult`
("Ref for synchronous access during sequential cell execution", `useNotebookVariables.ts:18`) that
`useCellExecution.ts` reads synchronously mid-execution (`:141-142`, `:293`), and `setVariableValue`'s
eager, same-tick write to it (`:195`) must keep working for that contract to hold — an existing test
already asserts it ("variableValuesRef is updated immediately after setVariableValue",
`useNotebookVariables.test.tsx:252`). So only the render-time *reads* (`:134-136`, `:149`) move from the
ref to the new `variableValues` state; the ref stays in the returned API and is resynced from that state
via a `useLayoutEffect` (`variableValuesRef.current = variableValues`, the Pattern 4 shape), so
`useCellExecution`'s synchronous contract is unaffected while the render-time write at `:154` moves to
`variableValues` state instead.

### `set-state-in-effect` — three sub-patterns, no single fix

Reproducing the false-positive/true-positive split needed one more empirical check: does the rule flag
*any* `setState` reachable from an effect, or only an unconditional call at the effect's top level?
Verified: a `setState` call nested inside a function that is *declared and invoked inside the effect
body* (sync or async) is not flagged, matching exactly the shape React's own docs use for data fetching in
an Effect (`useEffect(() => { async function run() { ...; setState(x) }; run() }, [])`). Only a direct
top-level call, or a call to a function declared *outside* the effect, is flagged. This distinguishes
three real categories among the 33 findings:

**(a) Fetch-on-mount via an already-extracted loader — 6 sites.** `Sidebar.tsx:105` (`loadFolders`),
`DataSourcesPage.tsx:65`, `ExportScreensPage.tsx:44`, `ImportScreensPage.tsx:98`, `MapsPage.tsx:67`,
`ScreensPage.tsx:77` all call a `useCallback`-memoized loader from a mount effect. Four of the six loaders
are genuinely reused elsewhere — `Sidebar.tsx:108`'s `useFoldersChangedListener(loadFolders)`, and
similarly `DataSourcesPage.tsx:108,121,133`, `MapsPage.tsx:81`, `ScreensPage.tsx:120,147` — so those can't
just be inlined into the effect without duplicating logic at the other call site. Fix: wrap the call in an
inline async IIFE inside the effect, with a one-line comment explaining why the wrap exists (a future
cleanup could otherwise mistake it for pointless indirection and delete it, silently re-breaking lint) —
verified clean, and it's the loader identity (not its body) doing the wrapping, so nothing else that
references the loader changes:

```tsx
useEffect(() => {
  // IIFE keeps the setState out of the effect's top level — see react-hooks/set-state-in-effect
  void (async () => {
    await loadFolders()
  })()
}, [loadFolders])
```

The other two — `ExportScreensPage.tsx`'s `loadData` (`:29`) and `ImportScreensPage.tsx`'s
`loadExistingScreens` (`:87`) — have exactly one caller, the mount effect itself (`:44`, `:98`), so they
take React's plain documented fix instead: move the loader's body directly into the effect, no IIFE, no
`useCallback`.

**(b) Fully-derived state duplicated via effect (should be `useMemo`, not `useState` + effect) — 8
sites, 5 files.** `ProcessPage.tsx:157,178,198`, `ProcessMetricsPage.tsx:268`, `useMetricsData.ts:106`,
`LogRenderer.tsx:329`, `ProcessLogPage.tsx:273`. All five share the identical shape: a `useEffect` gated on
`query.isComplete && !query.error` that reads `query.getTable()` — a pure, idempotent read of already-resolved
data — and writes it into a separate `useState`. There is no asynchrony left at this point (the actual
network wait already happened inside `query`'s own state machine); this is exactly "derived state that
belongs in render" (the rule's own stated rationale). Fix: replace the state + effect pair with a
`useMemo` keyed on the same guard condition, e.g. (`ProcessPage.tsx`'s `process`):

```tsx
const process = useMemo<ProcessRow | null>(() => {
  if (!processQuery.isComplete || processQuery.error) return null
  const table = processQuery.getTable()
  const row = table && table.numRows > 0 ? table.get(0) : null
  return row ? { exe: String(row.exe ?? ''), /* ...same fields... */ } : null
  // eslint-disable-next-line react-hooks/exhaustive-deps
}, [processQuery.isComplete, processQuery.error])
```

removing `setProcess`/the effect, but *keeping* the narrowed `[processQuery.isComplete, processQuery.error]`
deps and the `eslint-disable-next-line react-hooks/exhaustive-deps` comment guarding them: `useStreamQuery`
returns a fresh object every render (`useStreamQuery.ts:124`'s `{ ...state, execute, cancel, retry,
getTable, getBatches }`), so keying the memo on `processQuery` itself would invalidate it — and every
downstream memo keyed on its result (`classifyLogColumns`/`computeFlexWidths` in `LogRenderer.tsx:335-345`,
`availablePropertyKeys` in `useMetricsData.ts:115-121`) — on every render. `exhaustive-deps` is only
`"warn"` in `reactHooks.configs.recommended`, so the narrowed deps plus the disable comment don't fail
`yarn lint`. `LogRenderer.tsx:329` sets two states (`resultTable`, `hasLoaded`); only `resultTable` is
fully-derived and converts to `useMemo` as above. `hasLoaded` is a monotonic latch (it gates the
filter-change re-execute effect at `:417-422` and the initial spinner at `:575`), not state derivable from
`resultTable`: `resultTable !== null || (streamQuery.isComplete && streamQuery.error != null)` would go
back to `false` after a successful *empty* query (`getTable()` returns `null` when there are no batches,
even though the query succeeded) and would flip `true` after an error, neither of which matches today's
behavior. Instead `hasLoaded` becomes its own `useMemo` derived straight from the query's completion
signal, `streamQuery.isComplete && !streamQuery.error`, which reproduces today's semantics exactly (true
once a successful load lands, never true after an error) without depending on `resultTable`'s nullness.
`ProcessLogPage.tsx:273` has the same shape: its effect sets `rows` (fully-derived, converts to `useMemo`)
alongside `setHasLoaded(true)` at `:278`, which gates two re-execute effects (`:386-395`, `:400-409`) and
the spinner (`:580`) — the same monotonic-latch treatment applies: its own `useMemo` derived from the
query's completion signal, not from `rows`.

`ProcessMetricsPage.tsx:227` looks superficially like this shape but isn't — its effect performs a
router write (`updateConfig(..., { replace: true })`) and a conditional auto-selection
(`setSelectedMeasure(autoMeasure)`) alongside `setMeasures(measureList)` and `setDiscoveryDone(true)`,
none of which is a pure derived-value assignment. It gets the same inline-function wrap as sub-pattern (a)
below (the rule doesn't care what the wrapped body does, only that the `setState` calls sit inside a
function declared and invoked inside the effect) rather than a `useMemo`. Only `ProcessMetricsPage.tsx:268`
is a genuine (b) `useMemo` candidate there.

**(c) Local editable/UI state reset when a prop or dependency changes — 18 sites.** The remainder:
`Sidebar.tsx:113,126,137`, `MetricsRenderer.tsx:82,89`, `ImageCell.tsx:43,55`, `CellEditor.tsx:52`,
`XYChart.tsx:594`, `CustomRange.tsx:36`, `useDataSourceState.ts:21`, `NotebookRenderer.tsx:165`,
`HorizontalGroupCell.tsx:296`, `LogCell.tsx:207`, `PerfettoExportCell.tsx:53`, `TableCell.tsx:48`,
`pagination.tsx:50`, plus `useFadeOnIdle.ts:17` and `RecentRanges.tsx:10` (see below). These all mirror or
reset local state in response to a prop/dependency change (e.g. `CellEditor.tsx:52` resets the edited-name
input when `cell.name` changes; `pagination.tsx:50` clamps the current page when `totalRows`/`pageSize`
changes). Fix: React's documented "adjust state when a prop changes" pattern — track the previous value of
the triggering prop in state, and branch on the comparison directly in the render body (verified clean,
same mechanism as Pattern 5 above):

```tsx
const [editedName, setEditedName] = useState(cell.name)
const [nameError, setNameError] = useState<string | null>(null)
const [prevCellName, setPrevCellName] = useState(cell.name)
if (cell.name !== prevCellName) {
  setPrevCellName(cell.name)
  setEditedName(cell.name)
  setNameError(null)
}
```

`RecentRanges.tsx:10` is a variant — a mount-only read of `localStorage` via `getRecentTimeRanges()`, no
reactive dependency at all — for which a lazy `useState` initializer is simpler than the render-time
comparison and equally clean under the rule: `const [recentRanges] = useState(() =>
getRecentTimeRanges())`. `useFadeOnIdle.ts:17` sets state as part of a genuine timer-driven state machine
(`setTimeout` cleanup, not a plain prop mirror) — this one is a legitimate Effect, so it takes the (a)-style
inline-function wrap rather than a render-time rewrite: wrap the existing effect body in a nested function
declared and invoked *and returned* — `const run = () => { /* existing body, including both early
`return`s */ }; return run()`, not a bare invocation — so the effect's cleanup function
(`() => clearTimeout(id)`, `:26`) and its early returns still propagate out of the effect callback. As with
the (a) IIFEs, add a one-line comment at the wrap site stating why it's there.

### `refs` — fix what Patterns 4 and 5 cover, disable-with-reason for the rest

Patterns 4 and 5 above account for the two largest, mechanically-fixable clusters:
`useMapOrbitController.ts` (5), `PerspectiveCameraController.tsx`/`OrthographicCameraController.tsx` (2, via
Pattern 4), and `useNotebookVariables.ts` (11, via Pattern 5) — 18 of the 60. One more site needs a
small fix rather than an API addition: `PerspectiveCameraController.tsx:164-166`'s reset-view effect
directly assigns `sphericalRef.current.radius`/`.phi`/`.theta` — a ref *returned by*
`useMapOrbitController`, not owned locally. THREE.Spherical has its own setter method; replacing the three
property assignments with `sphericalRef.current.set(radius, phi, theta)` is a method call rather than a
direct property write, so the rule doesn't flag it (the adjacent `targetRef.current.copy(...)` on the next
line is already a method call and was never flagged in the first place). This also resolves the
corresponding `immutability` finding at the same lines (Design's Pattern 3 area — see Files to Modify),
since both findings only fire on the direct property assignments.

The remaining 42 sites are spread across 22 files, concentrated in canvas/imperative rendering code
(`XYChart.tsx`, `MapCell.tsx`, `NotebookRenderer.tsx`, `useScreenQuery.ts`, `ProcessPage.tsx`, and ~19 files
with 1-2 sites each). Per the issue's own framing and its acceptance criteria (the only rule of the five
allowed a per-site escape hatch), each gets triaged during implementation against the same two questions:

1. Is this a render-time ref *write* that's really the Pattern 4 "latest ref" shape? → move into an effect.
2. Is this a render-time ref *read* whose value could instead come from a prop, `useMemo`, or state? → do
   that instead.

If neither applies — e.g. a ref read that's genuinely needed synchronously during render to avoid a
one-frame flash in imperative Three.js/uPlot/canvas setup code, where deferring to an effect would change
visible behavior — add a scoped `// eslint-disable-next-line react-hooks/refs -- <reason ref read is safe
here>` at that exact line, per the issue's acceptance criterion. No blanket file- or rule-level disables.

## Implementation Steps

One branch, seven commits (four single-rule commits plus `refs` split into three — Pattern 4, the
`useNotebookVariables` refactor, then the long-tail triage — in the issue's suggested rule order), each
green on `python3 build/analytics_web_ci.py` before moving to the next so the branch bisects cleanly. The
first four commits and the last of the three `refs` commits each remove exactly one `'off'` line from
`analytics-web-app/eslint.config.js` (the last `refs` commit also removes the now-empty explanatory comment
block); the other two `refs` commits leave `eslint.config.js` untouched since the rule can't flip to
`'error'` until all its findings are resolved.

### Commit 1 — `react-hooks/purity`

1. `src/routes/PerformanceAnalysisPage.tsx:69` — apply Pattern 1.
2. Remove `'react-hooks/purity': 'off',` from `eslint.config.js`.
3. `yarn lint` shows zero `purity` findings; `python3 build/analytics_web_ci.py` green; commit.

### Commit 2 — `react-hooks/static-components`

1. `src/routes/ProcessesPage.tsx` — hoist `SortHeader` per Pattern 2a; update the 6 call sites.
2. `src/lib/screen-renderers/cells/HgChildPane.tsx` — `meta.renderer` per Pattern 2b.
3. `src/routes/ScreenPage.tsx` — `SCREEN_RENDERERS[screenType]` per Pattern 2b (import the export from
   `screen-renderers/index.ts`).
4. Remove `'react-hooks/static-components': 'off',`.
5. `yarn lint` zero findings for this rule; full CI; commit.

### Commit 3 — `react-hooks/immutability`

1. `src/lib/auth.tsx` — rewrite `checkAuth`'s recursion as a loop, per Pattern 3a. Run
   `src/lib/__tests__/auth.test.tsx` and `src/components/__tests__/AuthGuard.test.tsx` — behavior-preserving,
   should need no test changes.
2. `src/lib/screen-renderers/cells/FlameGraphCell.tsx` — `keyTickRef` per Pattern 3b. Run
   `FlameGraphCell.test.tsx`/`FlameGraphLayout.test.ts`.
3. `src/components/map/modes/PerspectiveCameraController.tsx:164-166` — replace the three direct property
   assignments (`sphericalRef.current.radius/.phi/.theta = …`) with `sphericalRef.current.set(radius, phi,
   theta)`, per the `refs` section above.
4. Remove `'react-hooks/immutability': 'off',`.
5. `yarn lint` zero findings for this rule; full CI; commit.

### Commit 4 — `react-hooks/set-state-in-effect`

1. Sub-pattern (a) — wrap each reused loader's mount-effect call in an inline async IIFE, with a one-line
   comment stating why the wrap exists (`Sidebar.tsx`, `DataSourcesPage.tsx`, `MapsPage.tsx`,
   `ScreensPage.tsx`); inline the loader body directly into the effect (no IIFE) for the two single-caller
   loaders (`ExportScreensPage.tsx`, `ImportScreensPage.tsx`).
2. Sub-pattern (b) — 5 files: replace state+effect with `useMemo` (`ProcessPage.tsx`,
   `ProcessMetricsPage.tsx`, `useMetricsData.ts`, `LogRenderer.tsx`, `ProcessLogPage.tsx`);
   `ProcessMetricsPage.tsx:227` is not (b)-shaped and instead gets the sub-pattern-(a) inline-function wrap
   (see Design); `LogRenderer.tsx`'s and `ProcessLogPage.tsx`'s `hasLoaded` latches each become their own
   `useMemo` derived from the query's completion signal, per the Design section above.
3. Sub-pattern (c) — remaining files: "adjust state when a prop changes" rewrite (`Sidebar.tsx`'s other 3
   sites, `MetricsRenderer.tsx`, `ImageCell.tsx`, `CellEditor.tsx`, `XYChart.tsx`, `CustomRange.tsx`,
   `useDataSourceState.ts`, `NotebookRenderer.tsx`, `HorizontalGroupCell.tsx`, `LogCell.tsx`,
   `PerfettoExportCell.tsx`, `TableCell.tsx`, `pagination.tsx`); lazy-init for `RecentRanges.tsx`;
   inline-function-wrap (returning the wrapper's invocation, with an explanatory comment) for
   `useFadeOnIdle.ts`.
4. Remove `'react-hooks/set-state-in-effect': 'off',`.
5. `yarn lint` zero findings for this rule; full CI (pay particular attention to
   `useMetricsData`/`ProcessMetricsPage`/`ProcessLogPage`/`LogRenderer` tests, since sub-pattern (b)
   changes state shape from settable to derived); commit.

### Commit 5 — `react-hooks/refs`, part 1: Pattern 4 mechanical moves

1. Apply Pattern 4 to `useMapOrbitController.ts`, `PerspectiveCameraController.tsx`,
   `OrthographicCameraController.tsx`.
2. `yarn lint` shows these 7 sites clear (the rule stays `'off'` in `eslint.config.js` — most `refs`
   findings are still unresolved); full CI; commit.

### Commit 6 — `react-hooks/refs`, part 2: `useNotebookVariables.ts` behavioral refactor

1. Apply Pattern 5 to `useNotebookVariables.ts`; run `useNotebookVariables.test.tsx` before and after —
   this is the one substantial behavioral refactor in the plan.
2. `yarn lint` shows these 11 sites clear (rule still `'off'`); full CI; commit.

### Commit 7 — `react-hooks/refs`, part 3: long-tail triage and rule flip

1. Triage the remaining 42 sites across the other 22 files per the two-question framework above; fix
   what's mechanically a Pattern 4/5 shape, add a justified `eslint-disable-next-line react-hooks/refs`
   comment at every site that isn't.
2. Remove `'react-hooks/refs': 'off',` and the now-fully-empty five-line comment block above it.
3. `yarn lint` zero *unjustified* findings (every remaining `react-hooks/refs` disable has a reason
   comment); full CI; manual smoke pass on the map viewer (orbit/fly camera controls, both perspective and
   orthographic modes) and a notebook screen with variables, per Testing Strategy; commit.

### Commit 8 (optional, docs-only) — changelog

Amend the existing `**Build:**` bullet at `CHANGELOG.md:27` (under `## Unreleased`) rather than appending a
second one — that bullet currently says the five React Compiler rules "are disabled for now; adopting them
is deferred and not yet tracked by an issue"; drop that clause and replace it with a note that the five
rules are now enabled at `error` with zero (or justified-disable) findings, referencing `(#1423)`. The
per-commit CI gate above does not apply to this commit.

## Files to Modify

**Commit 1**: `src/routes/PerformanceAnalysisPage.tsx`, `eslint.config.js`

**Commit 2**: `src/routes/ProcessesPage.tsx`, `src/lib/screen-renderers/cells/HgChildPane.tsx`,
`src/routes/ScreenPage.tsx`, `eslint.config.js`

**Commit 3**: `src/lib/auth.tsx`, `src/lib/screen-renderers/cells/FlameGraphCell.tsx`,
`src/components/map/modes/PerspectiveCameraController.tsx`, `eslint.config.js`

**Commit 4**: `src/components/layout/Sidebar.tsx`, `src/routes/DataSourcesPage.tsx`,
`src/routes/ExportScreensPage.tsx`, `src/routes/ImportScreensPage.tsx`, `src/routes/MapsPage.tsx`,
`src/routes/ScreensPage.tsx`, `src/routes/ProcessPage.tsx`, `src/routes/ProcessMetricsPage.tsx`,
`src/hooks/useMetricsData.ts`, `src/lib/screen-renderers/LogRenderer.tsx`, `src/routes/ProcessLogPage.tsx`,
`src/lib/screen-renderers/MetricsRenderer.tsx`, `src/lib/screen-renderers/cells/ImageCell.tsx`,
`src/components/CellEditor.tsx`, `src/components/XYChart.tsx`,
`src/components/layout/TimeRangePicker/CustomRange.tsx`,
`src/components/layout/TimeRangePicker/RecentRanges.tsx`, `src/hooks/useDataSourceState.ts`,
`src/hooks/useFadeOnIdle.ts`, `src/lib/screen-renderers/NotebookRenderer.tsx`,
`src/lib/screen-renderers/cells/HorizontalGroupCell.tsx`, `src/lib/screen-renderers/cells/LogCell.tsx`,
`src/lib/screen-renderers/cells/PerfettoExportCell.tsx`, `src/lib/screen-renderers/cells/TableCell.tsx`,
`src/lib/screen-renderers/pagination.tsx`, `eslint.config.js`

**Commit 5**: `src/components/map/hooks/useMapOrbitController.ts`,
`src/components/map/modes/PerspectiveCameraController.tsx`,
`src/components/map/modes/OrthographicCameraController.tsx`

**Commit 6**: `src/lib/screen-renderers/useNotebookVariables.ts`

**Commit 7**: a subset of the 22 files in the `refs` long tail (`NotebookRenderer.tsx`, `XYChart.tsx`,
`cells/MapCell.tsx`, `useScreenQuery.ts`, `ProcessPage.tsx`, `MapInstancedMarkers.tsx`, `LogRenderer.tsx`,
`useNotebookAutoRun.ts`, `warning-reporter.tsx`, `ProcessLogPage.tsx`, `ProcessMetricsPage.tsx`,
`ProcessesPage.tsx`, `PerformanceMetricsChart.tsx`, `useChangeEffect.ts`, `useRefreshInterval.ts`,
`useScreenConfig.ts`, `ProcessListRenderer.tsx`, `TableRenderer.tsx`, `cells/HgChildPane.tsx`,
`cells/TableCell.tsx`, `MeasureDiscovery.tsx`, `ThreadCoveragePanel.tsx`), `eslint.config.js`

**Commit 8**: `CHANGELOG.md`

## Trade-offs

- **Wrapping a loader call in an inline IIFE (sub-pattern (a)) vs. inlining the whole loader body into the
  effect.** Four of the six loaders (`Sidebar.tsx`'s `loadFolders`, `DataSourcesPage.tsx`'s,
  `MapsPage.tsx`'s, `ScreensPage.tsx`'s) are reused outside their mount effect (a change listener, a manual
  refresh path, etc.), so they can't be inlined without duplicating logic at the other call site — the IIFE
  wrapper is the minimal change that satisfies the rule without touching the loader's own definition or its
  other callers. The remaining two (`ExportScreensPage.tsx`'s `loadData`, `ImportScreensPage.tsx`'s
  `loadExistingScreens`) have no other caller, so they get the plain inline-into-the-effect fix instead —
  no IIFE needed there.
- **`PerspectiveCameraController`'s reset-view effect switches to `sphericalRef.current.set(...)` rather
  than `useMapOrbitController` gaining a new imperative API.** Broader alternative considered: make the
  hook not return raw refs at all, replacing every external read/write with accessor functions. Rejected as
  disproportionate — the rule only flags *direct property assignment* to a ref's contents, not method
  calls; the other writes to `targetRef`/`sphericalRef` from `PerspectiveCameraController` (the GLB-seed
  `useLayoutEffect`'s `.copy()`/`.setFromVector3()` calls, and `onRightDragReAnchor`'s
  `zoomAnchorTarget(targetRef.current, ...)`) are already method calls and stay untouched, so a one-line
  change to the three property writes is enough.
- **`refs`'s long tail ends up part-fixed, part-disabled**, per the issue's own acceptance criterion — the
  only one of the five rules where that's allowed. `purity`, `static-components`, `immutability`, and
  `set-state-in-effect` all reach zero findings with no disables anywhere.
- **`useNotebookVariables.ts`'s ref→state conversion (Pattern 5) is the one non-mechanical change in the
  whole plan.** It's also the largest single cluster (11 of 60 `refs` findings), so it can't be deferred to
  a disable without leaving the biggest finding in the issue unaddressed. Existing test coverage
  (`useNotebookVariables.test.tsx`) is the safety net; no new tests are planned unless that suite doesn't
  already exercise the recompute-on-change path being touched.
- **`welcome/` is out of scope here.** It's 9 `.ts`/`.tsx` source files on `eslint-plugin-react-hooks@^4.6.0`
  (`welcome/package.json:36-37`) and isn't linted in CI at all (`.github/workflows/publish-docs.yml` only
  builds it, `:56-58`) — its eventual v4→v7 migration is a much smaller job than this plan's, and the fix
  patterns in this plan's Design section are reusable when that happens.

## Testing Strategy

- Per-commit: `python3 build/analytics_web_ci.py` (type-check → lint → test → build) green.
- Per-commit: `yarn lint` (or the scratch-config repro technique used to write this plan) shows zero
  findings for that commit's rule, and — from Commit 7 on — that every remaining `react-hooks/refs`
  finding carries a disable comment with a stated reason, not that the rule is silent.
- Existing unit-test suites double as regression coverage for the two behavioral refactors:
  `src/lib/__tests__/auth.test.tsx` + `AuthGuard.test.tsx` (Pattern 3a), and
  `useNotebookVariables.test.tsx` (Pattern 5) — run before touching each file to record the current
  passing baseline, then after the refactor.
- Manual smoke pass (dev workflow per `analytics-web-app/README.md`: split-mode services +
  `start_analytics_web.py`) after Commit 7: map viewer orbit/pan/zoom/WASD-fly in both perspective and
  orthographic camera modes (exercises `useMapOrbitController`'s Pattern 4 rewrite and
  `PerspectiveCameraController`'s reset-view fix), and a notebook screen with a variable cell whose
  default/value tracking was touched by Pattern 5.
- Sub-pattern (b)'s `useMemo` conversions (Commit 4) change state shape from settable to derived; confirm
  no other code in the touched files calls the removed setters (a `grep -n 'setProcess\|setStatistics\|...'`
  sweep of each file being converted) before deleting them.
