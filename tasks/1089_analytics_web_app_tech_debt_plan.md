# analytics-web-app Tech-Debt Refactor Plan (#1089)

## Issue Reference
- [#1089](https://github.com/madesroches/micromegas/issues/1089) — Tech debt: refactor four high-payback areas in analytics-web-app

## Overview

`analytics-web-app/` is ~46K lines across ~200 files; four files concentrate
~5K lines and most of the cyclomatic complexity. This plan splits those files
along the seams where pure logic, THREE.js wiring, and React shells meet, so
the heavy parts become independently testable and the React components shrink
to readable shells. It also folds in six smaller cleanups surfaced by the same
audit.

Each numbered item below is a **self-contained PR**. They share no code, so
they can land in any order, but the suggested sequence (4 → 2 → 3 → cleanups)
puts the lowest-blast-radius change first and groups the graphics-heavy work.

**Scope note — Item 1 is out of scope.** The original tracker listed a
"state mutation during render" bug in `MapCell.tsx`. The issue author later
confirmed this is a **false positive**: the code uses React's canonical
"adjusting state during render" pattern, and the cross-component publish is
already correctly deferred. No change required.

**Guiding constraint for every item:** these are pure refactors. Behavior,
props, and rendered output must be identical before and after. The win is
structure and testability, not new features.

## Current State

All line numbers below reflect a structural reading at planning time. The
audit in #1089 cited older line numbers that have since drifted — **re-confirm
ranges against the file before editing.** The section boundaries (not the exact
lines) are what matter.

### Item 2 — `FlameGraphCell.tsx` (1267 lines)
`src/lib/screen-renderers/cells/FlameGraphCell.tsx`

A single file holding five distinct concerns:

| Concern | Approx. lines | React? | Notes |
|---|---|---|---|
| Constants + palette | 1–62 | no | `FLAME_PALETTE`, `SPAN_HEIGHT`, etc. |
| Data model `buildFlameIndex` + `LaneIndex`/`FlameIndex` + `axisValue`/`detectXAxisMode` | 79–230 | no | pure; already exported |
| Pure helpers: `laneYOffset`, `totalHeight`, `hitTest`, `spanColor`/`spanColorIndex`, `formatDuration`/`formatBits`/`formatAxisTick` | 66–329 | no | scattered around the file |
| `FlameGraphView` — THREE setup, render loop, Canvas2D overlay, all interaction handlers | 341–1006 | yes | the bulk; ~665 lines |
| `resolveInitialTimeRange`, `FlameGraphCell` shell, `FlameGraphCellEditor`, `flamegraphMetadata` | 1021–1266 | yes | React shell + cell-type registration |

The async-tree layout was **already extracted** to `FlameGraphLayout.ts`
(`computeAsyncVisualDepths`, imported at the top) — precedent for this split.

The "worst stretch" the issue calls out (a ~200-line layout/instancing loop)
is the `render()` callback inside `FlameGraphView` (~376–574): it allocates
the InstancedMesh capacity, fills per-instance matrices/colors for visible
spans, then draws labels/headers/axis/selection on the Canvas2D overlay.

### Item 3 — `MapViewer.tsx` (1086 lines)
`src/components/map/MapViewer.tsx`

| Concern | Approx. lines | React? | Notes |
|---|---|---|---|
| Types + constants | 1–103 | no | `MapViewerProps`, color/scale constants |
| Coordinate math: `sphericalToZUpOffset`, `zUpOffsetToSphericalInput`, `cameraBasisFromSpherical` | 471–511 | no | pure; `cameraBasisFromSpherical` already exported |
| GLSL shader patch `patchInstanceColorRGBA` (`onBeforeCompile`) | 111–135 | no | raw GLSL strings; each `.replace()` already names its chunk inline, but no explanatory comment on why each block overrides it |
| `MapModel` — GLTF load, mesh traversal, camera/light extraction | 52–89 | yes | drei `useGLTF` |
| `InstancedMarkers` — three-pass instancing (matrix / color baseline / highlight diff), interaction | 137–464 | yes | per-instance buffers in refs |
| `MapCameraController` — orbit state, GLB camera seeding, mouse/keyboard/wheel handlers, `useFrame` loop | 520–927 | yes | largest single block (~400 lines); `panCamera` (709–727) is a closure that reads `sphericalRef.current` and mutates `targetRef.current` — its math must be extracted into a pure function, not moved as-is |
| `SceneSetup`, `MapViewer` container | 929–1085 | yes | Canvas, Suspense, ready-gate |

### Item 4 — `PerformanceAnalysisPage.tsx` (1001 lines)
`src/routes/PerformanceAnalysisPage.tsx`

Three independent concerns share one big `PerformanceAnalysisContent`
component (132–975) and a flat pile of ~20 `useState` + ~20 `useCallback`
hooks. There is **no explicit reducer** — the coupling is that all three
concerns read from the same component-scoped state and the same effect block:

- **Discovery** — `DISCOVERY_SQL`, `loadDiscovery`, measure dropdown, auto-select.
- **Metrics chart** — `useMetricsData` hook + custom-query path, `MetricsChart`, property timeline, axis bounds.
- **Thread-coverage timeline** — `THREAD_COVERAGE_SQL` + `TRACE_EVENTS_COUNT_SQL`, `loadThreadCoverage`, Perfetto trace open/download (`handleOpenInPerfetto`, `handleDownloadTrace`, cached-buffer logic).

Pure/extractable bits: `buildUrl` (config→URLSearchParams, 79–95),
`calculateBinInterval` (103–130), SQL templates + `VARIABLES` (29–66),
`DEFAULT_CONFIG` (69–76).

### Lower-priority files
- `src/components/XYChart.tsx` (996 lines) — giant `useEffect` building axis + series config inline.
- `src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` (820 lines) — drag-drop layout + nested cell renderer inline.
- `CellTypeMetadata` shape repeated in `TableCell.tsx:291`, `TransposedTableCell.tsx:206`, `ReferenceTableCell.tsx:167`.
- `Table.get()` materialization loop (`Array.from({ length: … })`) appears **8×** across `src/lib/screen-renderers/`.
- `useColumnManagement` (`table-utils.tsx:863`) and `useRowManagement` (`table-utils.tsx:943`) — near-identical.
- Regex factories the audit cited in `notebook-utils.ts:303-312` — **already addressed.** The factories live in `macro-substitution.ts`, are already collapsed into a single shared definition (~lines 41-51), and already carry a comment documenting the fresh-RegExp pattern (~lines 37-39). (Line 137 is a `new RegExp(...)` inside the substitution loop, not a factory.) Verify-only; no action expected.

## Design

The unifying principle: **extract the pure logic out from under the THREE.js /
React layers, then let the component re-import it.** A pure module takes plain
inputs (Arrow `Table`, numbers, vectors) and returns plain outputs; it imports
no React and, where possible, no THREE primitives beyond math types. That makes
it unit-testable with the existing Jest setup and shrinks each component to
wiring.

This satisfies the open/closed principle (the extracted layout/scene modules
can be extended without reopening the React shell) and DRY (the cleanups
collapse repeated shapes into one factory/helper).

### Item 2 — FlameGraphCell split

Target end state — four files in `cells/`:

```
FlameGraphLayout.ts   (exists) async-tree depths
flame-model.ts        buildFlameIndex, LaneIndex, FlameIndex, axisValue,
                      detectXAxisMode, laneYOffset, totalHeight, hitTest,
                      spanColor, spanColorIndex, formatDuration, formatBits,
                      formatAxisTick   ── all pure, all unit-testable
FlameGraphScene.ts    THREE setup (renderer, ortho camera, InstancedMesh,
                      ResizeObserver) + the render() function that fills
                      instance matrices/colors and draws the Canvas2D overlay.
                      Exposes a small imperative handle:
                        createFlameScene(container, canvases) → {
                          render(index, viewState), resize(), dispose() }
FlameGraphCell.tsx    React shell (~400 lines): FlameGraphView wires the scene
                      handle to refs + interaction handlers + tooltip; plus
                      resolveInitialTimeRange, FlameGraphCell, the editor, and
                      flamegraphMetadata.
```

Key boundary decision: **interaction handlers stay in the React shell.** They
mutate `stateRef` (viewMinTime/Max, scrollY, isDragging) and call
`requestRender()`; they are inherently coupled to the component's ref-held view
state and the rAF loop. Moving them would require threading a callback bag
through the scene module for little gain. What moves into `FlameGraphScene` is
only the stateless "given this view state + index, paint a frame" logic.

`buildFlameIndex` and `formatBits` are exported but consumed only by the test
file (`cells/__tests__/FlameGraphCell.test.tsx`); no production code imports
them — keep the same export names and repoint the test import. No re-export
shim needed.

### Item 3 — MapViewer split

Target end state — files under `src/components/map/`:

```
shader-patches.ts       patchInstanceColorRGBA + the GLSL blocks as named
                        template literals, EACH prefixed with a comment naming
                        the `#include <chunk>` it overrides (e.g.
                        "// overrides #include <begin_vertex>"). This is the
                        single doc-debt the issue explicitly calls out.
map-camera-math.ts      sphericalToZUpOffset, zUpOffsetToSphericalInput,
                        cameraBasisFromSpherical, the cursor-anchored
                        zoom-target computation ── pure, unit-testable.
                        ALSO: extract panCamera's math into a pure helper taking
                        (spherical, deltaX, deltaY) and mutating a passed-in
                        target vector (or returning the offset). The existing
                        panCamera closure reads sphericalRef.current and mutates
                        targetRef.current, so it must be reshaped to take those
                        as explicit parameters — it is NOT pure as written.
MapCamera.tsx           MapCameraController component: event binding, GLB-camera
                        seeding, useFrame loop, raycasting. Imports map-camera-math.
MapInstancedMarkers.tsx InstancedMarkers component: three-pass instancing,
                        interaction handlers. Imports shader-patches.
MapViewer.tsx           Container only: MapModel, SceneSetup, Canvas, Suspense,
                        ready-gate, error banner, control hints.
```

`cameraBasisFromSpherical` is already exported from `MapViewer.tsx`; move it to
`map-camera-math.ts` and re-export from `MapViewer.tsx` if an external importer
exists (grep first). The three-pass instancing architecture and its ref-held
buffers (`colorAttrRef`, `runtimeColorsRef`, `prevHighlightRef`) move
**wholesale** into `MapInstancedMarkers.tsx` — do not try to extract the diff
logic into a pure module in this PR; the buffers are intrinsically stateful and
the win is just moving them out of the container file.

### Item 4 — PerformanceAnalysisPage split

Target end state — `PerformanceAnalysisPage.tsx` becomes a thin layout
container that mounts three sibling components, each owning its own state slice:

```
perf-analysis/
  queries.ts                  SQL templates, VARIABLES, DEFAULT_CONFIG, buildUrl,
                              calculateBinInterval  ── pure
  MeasureDiscovery.tsx        discovery query + measure dropdown; owns measures,
                              selectedMeasure, discoveryLoading/Done. Emits the
                              selected measure up via callback/config.
  PerformanceMetricsChart.tsx useMetricsData + custom-query path + MetricsChart +
                              property timeline + axis bounds.
  ThreadCoveragePanel.tsx     CONTROLLER: thread-coverage query + Perfetto
                              open/download + cached-buffer logic; builds the
                              ThreadCoverage[] and feeds the existing
                              ThreadCoverageTimeline VIEW via its `threads` prop.
                              (ThreadCoverageTimeline.tsx is already purely
                              presentational — props threads/timeRange/axisBounds/
                              onTimeRangeSelect, local drag-select state only, zero
                              data fetching — so it stays untouched.)
PerformanceAnalysisPage.tsx   AuthGuard + Suspense + useScreenConfig + layout.
                              Holds only shared cross-cutting state: time range,
                              processId, the config object that drives the URL.
```

The hard part is the **shared time range / config**. Today all three concerns
read `apiTimeRange`, `processId`, and the `useScreenConfig` config from the same
scope. After the split, those stay in the page container and pass **down as
props**; each child re-queries off its own effect when the time range prop
changes. **Caveat — the time-range re-fetch is currently gated on the metrics
chart's completion:** the single time-range-change effect
(`PerformanceAnalysisPage.tsx:502–518`) guards on `hasLoaded`
(`= metricsData.isComplete` from the metrics chart) and only then fires BOTH
`loadDiscovery()` AND `loadThreadCoverage()`. The `hasLoadedDiscoveryRef`
(line 479) is likewise a single ref shared across discovery + thread-coverage,
reset together in `handleRefresh` (531–535). Giving each child its own
independent guard would decouple discovery/thread-coverage re-fetch from
`metricsData.isComplete` and change fetch timing — violating the "behavior
identical" constraint. So the **page container must keep owning this gate**:
lift `isComplete`/load state up to the page and have it orchestrate the
re-fetch, rather than each child holding an independent
`prevTimeRangeRef`/`hasLoaded` guard.

This is the only item with a real correctness risk: the current effect
orchestration (`hasLoadedDiscoveryRef`, the metrics-execute-after-discovery
gate, the time-range-change reset) is subtle. **Preserve the exact ordering**:
discovery completes → selectedMeasure set → metrics fetch fires. After the
split this becomes: page passes `processId`/`timeRange` to `MeasureDiscovery`,
which lifts the selected measure to the page, which passes it to
`PerformanceMetricsChart`. Verify no double-fetch and no fetch-before-discovery
regression (see Testing Strategy).

### Lower-priority cleanups (Item 5, one PR)

- **`createQueryCellMetadata` factory.** Add to `cell-registry.ts`:
  `createQueryCellMetadata({ renderer, editor, label, icon, sqlKey, … })`
  returning a `CellTypeMetadata`. Collapse the three duplicated literals in
  `TableCell.tsx`, `TransposedTableCell.tsx`, `ReferenceTableCell.tsx`. Diff the
  three current shapes first to confirm they're truly identical modulo those
  fields; if `execute`/`getRendererProps` differ, keep those as passed-in
  callbacks.
- **`materializeTable(table): Record<string, unknown>[]`** in `table-utils.tsx`
  (or a new `table-utils` helper section). Replace the 8 `Array.from({ length })`
  + `table.get(i)` loops. Confirm all 8 sites want the same null-handling.
- **Extract `useHiddenList` from `useColumnManagement` / `useRowManagement`**
  (`table-utils.tsx:863,943`). Not a full merge — see Open Questions for the
  diff. Add a generic `useHiddenList(config, fieldKey, onChange)` for the
  hide/restore/restore-all-over-a-string-array body; `useRowManagement` becomes
  a thin field-renaming wrapper; `useColumnManagement` reuses it for restore
  handlers and keeps its sort handlers + sort-clearing `handleHideColumn`.
- **`buildXAxisConfig()` / `buildChartSeries()`** extracted from the big
  `useEffect` in `XYChart.tsx`, shrinking it to orchestration.
- **`HorizontalGroupCell.tsx`** — extract drag-drop layout and the nested cell
  renderer into sibling helpers/components.
- **`macro-substitution.ts` regex factories** — likely a no-op: the factories
  are already collapsed into one shared definition and carry an ordering comment.
  Verify they're still DRY and documented; only act if a regression has crept in.

## Implementation Steps

Each phase is its own PR and its own branch off `main`.

### PR A — Item 4: PerformanceAnalysisPage split (do first, lowest blast radius)
1. Create `src/routes/perf-analysis/queries.ts`; move SQL templates,
   `VARIABLES`, `DEFAULT_CONFIG`, `buildUrl`, `calculateBinInterval`. Export.
2. Create `MeasureDiscovery.tsx`; move discovery state/query/dropdown. Define
   its props (`processId`, `timeRange`, `onMeasureSelected`).
3. Create `PerformanceMetricsChart.tsx`; move `useMetricsData`, custom-query
   path, `MetricsChart`, property timeline, axis bounds.
4. Create the thread-coverage owner component; move thread-coverage + trace
   open/download + cached-buffer logic.
5. Reduce `PerformanceAnalysisPage.tsx` to AuthGuard + Suspense +
   `useScreenConfig` + layout; thread shared `timeRange`/`processId`/config down.
   Keep the time-range re-fetch gate in the page: lift the metrics chart's
   `isComplete` and the shared `hasLoadedDiscoveryRef` up so the page still
   gates discovery + thread-coverage re-fetch on metrics completion (do NOT give
   each child an independent guard — see Item 4 design caveat).
6. Add unit tests for `queries.ts` (`calculateBinInterval`, `buildUrl`).
7. Lint, type-check, test, then manual smoke (effect-ordering check).

### PR B — Item 2: FlameGraphCell split
1. Create `flame-model.ts`; move all pure functions/types listed above, keeping
   the `buildFlameIndex`/`formatBits` export names. The only out-of-file
   importer is `cells/__tests__/FlameGraphCell.test.tsx:2` (no production code
   imports them) — repoint that test import to `../flame-model`. No re-export
   shim needed.
2. Create `FlameGraphScene.ts`; move THREE setup + `render()` behind a
   `createFlameScene(...)` imperative handle.
3. Slim `FlameGraphCell.tsx` to the React shell wiring the scene handle to refs
   + interaction handlers + tooltip + editor + metadata.
4. Add unit tests for `flame-model.ts` (`buildFlameIndex`, `hitTest`,
   `spanColorIndex`, `formatDuration`/`formatBits`).
5. Lint, type-check, test, manual smoke.

### PR C — Item 3: MapViewer split
1. Create `shader-patches.ts` with documented GLSL blocks.
2. Create `map-camera-math.ts`; move pure coordinate/pan/zoom math. The only
   out-of-file importer of `cameraBasisFromSpherical` is
   `map/__tests__/MapViewer.test.tsx:2` (no production code imports it) —
   repoint that test import to `../map-camera-math`. No re-export shim needed.
3. Create `MapCamera.tsx` (controller) and `MapInstancedMarkers.tsx`.
4. Slim `MapViewer.tsx` to the container.
5. Add unit tests for `map-camera-math.ts` (round-trip
   `sphericalToZUpOffset`/`zUpOffsetToSphericalInput`, basis orthonormality).
6. Lint, type-check, test, manual smoke (load a map, orbit/pan/zoom, select).

### PR D — Item 5: lower-priority cleanups
Pick up opportunistically; each sub-bullet can also fold into a PR already
touching that file. Order within the PR: metadata factory → `materializeTable`
→ management-hook merge → XYChart → HorizontalGroupCell → `macro-substitution.ts` regex-factory verify (likely no-op).
Lint/type-check/test after each.

## Files to Modify

**PR A:** `src/routes/PerformanceAnalysisPage.tsx` (shrink) + new
`src/routes/perf-analysis/{queries.ts,MeasureDiscovery.tsx,PerformanceMetricsChart.tsx, + thread-coverage owner}.tsx`; new tests under `__tests__/`.

**PR B:** `src/lib/screen-renderers/cells/FlameGraphCell.tsx` (shrink) + new
`flame-model.ts`, `FlameGraphScene.ts` in same dir; new `__tests__/flame-model.test.ts`.

**PR C:** `src/components/map/MapViewer.tsx` (shrink) + new `shader-patches.ts`,
`map-camera-math.ts`, `MapCamera.tsx`, `MapInstancedMarkers.tsx`; new
`__tests__/map-camera-math.test.ts`.

**PR D:** `src/lib/screen-renderers/cell-registry.ts`,
`cells/{TableCell,TransposedTableCell,ReferenceTableCell}.tsx`,
`table-utils.tsx`, `components/XYChart.tsx`,
`cells/HorizontalGroupCell.tsx`, `macro-substitution.ts`.

## Trade-offs

- **Extract pure logic vs. move whole components into new files.** For
  FlameGraph and Map we do both: math/model becomes pure and testable, while
  stateful THREE wiring (instancing passes, rAF loop, ref-held view state) just
  *moves* to a dedicated file without being decomposed further. Trying to make
  the instancing/render loop "pure" would invert control through callback bags
  for no testability gain — the value there is locality, not purity.
- **Interaction handlers stay in the React shell (FlameGraph).** They are
  coupled to `stateRef` + `requestRender`; extracting them would leak the
  component's internals. Accepted: the shell stays ~400 lines, not ~150.
- **Item 4 prop-threading vs. context/reducer.** Could introduce a context or a
  `useReducer` for the shared time range. Rejected for this refactor:
  prop-threading keeps the dependency graph explicit and matches the existing
  `useScreenConfig` URL-driven pattern; a reducer would be a behavior-shaped
  change layered on top of a structural one, raising blast radius.
- **Sequencing 4 → 2 → 3.** Item 4 touches no graphics code (safest first);
  2 and 3 are graphics-heavy and should be coordinated with whoever last
  touched rendering, so they go later. Order is advisory — items are
  independent.
- **Lower-priority merges are conditional.** `useColumnManagement`/`useRowManagement`
  and the regex factories are merged only if inspection confirms the audit's
  "near-identical" claim; otherwise document the difference and skip. Forcing a
  merge on superficially-similar-but-semantically-different code is worse than
  the duplication.

## Documentation

No user-facing docs change — these are internal refactors with identical
behavior. Two internal-doc touch-ups:
- `shader-patches.ts` — each GLSL block's target chunk is already visible (it's
  the `.replace()` first arg); add an explanatory comment above each block on
  *why* it overrides that chunk (the doc-debt the issue calls out).
- The `macro-substitution.ts` regex factories already carry their ordering comment;
  no doc change expected unless the verify step finds it missing.

No CLAUDE.md / AI_GUIDELINES.md changes.

## Testing Strategy

The bar for every PR: **prove behavior is unchanged.**

- **New unit tests** on each extracted pure module (`flame-model.ts`,
  `map-camera-math.ts`, `perf-analysis/queries.ts`) — these are the payoff of
  the split and the regression net. Use the existing Jest ESM setup; mock via
  `jest.mock(...)` factories, never `jest.spyOn` on module namespaces (this
  repo's Jest runs ESM, where namespace exports are read-only — see the
  `1092_override_cell_memo_fix_plan.md` note).
- **Existing tests** must stay green unchanged (`FlameGraphCell` / map / page
  tests under `__tests__/`). If an existing test imports a now-moved symbol,
  fix the import only — do not change assertions.
- `yarn lint && yarn type-check && yarn test` per PR.
- **Manual smoke per PR** (`yarn dev`, or `./start_analytics_web.py`):
  - PR A: open Performance Analysis page → discovery populates dropdown →
    measure auto-selects → metrics chart renders → thread-coverage timeline
    renders → change time range and confirm exactly one re-fetch per concern,
    no fetch-before-discovery, no double-fetch. Open/download a Perfetto trace.
  - PR B: open a flame-graph cell → spans render, hover tooltip, drag-zoom,
    WASD pan/zoom, double-click reset, alt-drag time-range select.
  - PR C: open a map cell → markers render, orbit (right-drag), pan (left-drag),
    ctrl+wheel zoom, click-select, hover highlight, reset view.

## Open Questions

None blocking — the items below were investigated and resolved against the repo:

- **`useColumnManagement`/`useRowManagement` (resolved: extract shared body, do
  not fully merge).** Diffed both (`table-utils.tsx:863` and `:943`). They are
  *not* interchangeable: `useColumnManagement` additionally owns all the sort
  logic (`handleSort`/`handleSortAsc`/`handleSortDesc`) and its `handleHideColumn`
  has a sort-clearing side effect (clears `sortColumn`/`sortDirection` when the
  hidden column was the sorted one) that `useRowManagement` has no equivalent of.
  The genuinely shared part is only the hide/restore/restore-all over a named
  `hidden<X>` string array. Plan: extract a generic
  `useHiddenList(config, fieldKey, onChange) → { hidden, handleHide,
  handleRestore, handleRestoreAll }`; `useRowManagement` becomes a thin wrapper
  renaming the fields; `useColumnManagement` reuses it for the restore handlers
  but keeps its own sort-clearing `handleHideColumn` plus the sort handlers. A
  single unified hook is rejected — the differing config keys and sort coupling
  make it more awkward than the shared-body extraction.
- **Re-export shims (resolved: none needed).** Grepped the repo: the only
  out-of-file importers of `buildFlameIndex`/`formatBits` and
  `cameraBasisFromSpherical` are their respective test files. No production code
  imports them, so the moves repoint the test imports — no shims.
- **PR D granularity (default: one PR).** Treated as a single cleanup PR; split
  per sub-item only if it grows unwieldy. Process preference, not a blocker.

## Execution Log

Landing all four items on the `refac` branch (no separate per-PR branches), with
a commit between each item. Tests added wherever a safety net was warranted.

### PR A — DONE (type-check + lint + 1013-test suite all green)
Split `PerformanceAnalysisPage.tsx` (1000 → 463 lines) into:
- `perf-analysis/queries.ts` — pure SQL/`VARIABLES`/`DEFAULT_CONFIG`/`buildUrl`/
  `calculateBinInterval`/`Measure`. Unit-tested (`__tests__/queries.test.ts`).
- `perf-analysis/usePerfettoTrace.ts` — self-contained trace-generation hook
  (open/download/cached-buffer + progress/error state). The page renders the
  SplitButton and trace banners from it.
- `perf-analysis/MeasureDiscovery.tsx` — discovery query + measure dropdown.
- `perf-analysis/PerformanceMetricsChart.tsx` — `useMetricsData` + custom-query
  path + the metrics-execute effect + the chart-area states.
- `perf-analysis/ThreadCoveragePanel.tsx` — thread-coverage + event-count queries,
  renders the bottom `ThreadCoverageTimeline`.

**Architecture note (deviation from the literal plan, kept behavior-identical):**
the page's three concerns are entangled through the shared re-fetch gate
(`hasLoadedDiscoveryRef` + the `hasLoaded`-gated time-range effect) AND the layout
interleaves them (measure dropdown + Perfetto button share one toolbar row; trace
banners render between toolbar and chart; the coverage timeline is at the bottom).
A clean "each child owns and re-queries its own slice independently" split would
change fetch timing, which the plan forbids. So:
- **State that the gate or multiple sections read stays page-owned** (`selectedMeasure`,
  `measures`, `discoveryDone`/`Loading`, `queryError`, `traceEventCount`,
  `chartWidth`/`chartAxisBounds`). Each component encapsulates its *fetch logic +
  JSX + genuinely-local state* and **registers its loader into a page-held ref**, so
  the page's two gate effects call the loaders exactly as before.
- The metrics-execute effect moved into `PerformanceMetricsChart`; it lifts the
  gate-relevant view-state (`hasLoaded`, `isLoading`, `chartTimeRange`,
  `chartDataLength`, `propertyParseErrors`) back to the page via a single stable
  callback, computed from memo-stable values so there is no render loop.
- **Safety net:** added `__tests__/PerformanceAnalysisPage.test.tsx`
  characterizing the gate (mount fetch order + auto-select + metrics execute;
  one re-fetch per concern on time-range change, gated on metrics completion;
  refresh re-fetch). Written against the pre-refactor code first (green), then
  kept green after the split.

### PR B — DONE (type-check + lint + 1022-test suite all green)
Split `FlameGraphCell.tsx` (1266 → 727 lines) into:
- `flame-model.ts` (319) — pure: `axisValue`/`detectXAxisMode`, palette +
  `spanColor`/`spanColorIndex`, constants, `LaneIndex`/`FlameIndex`,
  `buildFlameIndex`, `laneYOffset`/`totalHeight`, `hitTest`, `formatDuration`/
  `formatBits`/`formatAxisTick`. No THREE, no React.
- `FlameGraphScene.ts` (287) — owns the WebGL resources (renderer, ortho
  camera, instanced mesh) behind `createFlameScene(webglCanvas, textCanvas,
  initialCapacity) → { resize(w,h,dpr), render(index, view), dispose() }`.
  The render body (instancing + Canvas2D overlay) is a verbatim port that now
  takes the view snapshot as a parameter instead of reading a shared ref.
- `FlameGraphCell.tsx` — React shell: `FlameGraphView` keeps all view +
  interaction state in a ref and the rAF loop, snapshots it into
  `scene.render(index, view)`; plus `resolveInitialTimeRange`, the cell, the
  editor, and `flamegraphMetadata` (still the only export `cell-registry`
  imports).

The test's `buildFlameIndex`/`formatBits` import was repointed to
`../flame-model`; added `__tests__/flame-model.test.ts` covering
`spanColor(Index)`, `formatDuration`, `laneYOffset`/`totalHeight`, `hitTest`.
Note: no production code imported the moved symbols (only the test), so no
re-export shim was needed.

### PR C — pending
### PR D — pending
