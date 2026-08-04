# Chart Infinity Value Handling Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1424

## Overview

A single non-finite value (`Infinity`/`-Infinity`) in a chart's X or Y column — e.g. from a SQL
ratio that divides by zero for some rows — currently survives the data-extraction stage and
poisons the whole chart's auto-scaled Y-axis, collapsing every other point/bar to a sliver near
zero. This plan tightens the existing null/NaN filtering in `arrow-utils.ts` to also drop
non-finite values, matching how `null` rows are already skipped. It also covers a second,
independent extraction path — the perf-analysis metrics chart's own row extraction in
`useMetricsData.ts` and `PerformanceMetricsChart.tsx` — which feeds the same downstream
`XYChart.tsx` internals but currently has no null/NaN/finiteness guard at all, and is reachable
today via arbitrary user-typed SQL (the custom-query editor). A third, independent extraction
path — `ProcessMetricsPage.tsx`'s own "Unified extraction effect" — has the identical unguarded
pattern and is likewise reachable via arbitrary user-typed SQL (its own custom-query editor).

## Current State

`analytics-web-app/src/lib/arrow-utils.ts` has two data-extraction entry points,
`extractMultiSeriesChartData` (`arrow-utils.ts:336`) and `extractChartData` (`arrow-utils.ts:529`),
each with categorical and time/numeric code paths. All four paths follow the same pattern: skip
the row if `xVal == null || yVal == null`, convert to `Number(...)`, then skip the row with
`isNaN(...)`. Since `isNaN(Infinity) === false`, a non-finite value passes this guard and is
pushed into the series as a normal `ChartPoint`.

The six guard sites (`isNaN` checks over an X and/or Y numeric value derived from a row):

- `arrow-utils.ts:424` — `extractMultiSeriesChartData`, categorical path, Y only
- `arrow-utils.ts:441` — `extractMultiSeriesChartData`, time/numeric path, X and Y
- `arrow-utils.ts:501` — `extractMultiSeriesChartData`, categorical label-remap pass, Y only
- `arrow-utils.ts:567` — `extractChartData`, categorical path, Y only
- `arrow-utils.ts:613` — `extractChartData`, time/numeric path, X and Y

Downstream, `analytics-web-app/src/components/XYChart.tsx:146` (`computeStats`) computes
`min`/`max`/`avg`/`p99` over each series' Y values with a plain sort/reduce — no finiteness check —
so an `Infinity` that reaches this point becomes the series' `max`/`p99`, and the Y scale's
`range()` callback (which multiplies by 1.05) then produces an infinite axis bound. Note
`XYChart.tsx:376` already guards a *different* value (a computed `rawValue`, likely a reference
line or derived overlay) with `Number.isFinite`, confirming `Number.isFinite` is the pattern
already in use elsewhere in this file for exactly this class of problem.

## Design

Replace `isNaN(n)` with `!Number.isFinite(n)` at each of the five call sites listed above. This
is a strict superset of the existing check — `Number.isFinite` returns `false` for `NaN`,
`Infinity`, and `-Infinity` alike, and `true` for every value `isNaN` currently accepts — so
existing null/NaN-skipping behavior and its tests are preserved, and non-finite values are now
skipped identically to how `null` values already are.

No changes are needed to `ChartPoint`, `computeStats`, or the Y-scale range calculation in
`XYChart.tsx`: once non-finite values can no longer enter a series' data array, `computeStats`
never sees an `Infinity` to propagate.

**Second extraction path: perf-analysis metrics chart.** `PerformanceMetricsChart.tsx` renders
via `MetricsChart` → `TimeSeriesChart` (a thin alias, `XYChart.tsx:1253`) → the same
`computeStats` (`XYChart.tsx:146`) and `range()` (`XYChart.tsx:726-731`) identified above, but its
chart points are built independently of `arrow-utils.ts` and have no guard whatsoever:

- `useMetricsData.ts:93` (the unified measures-query effect) and
  `PerformanceMetricsChart.tsx:188` (`loadCustomQuery`, the custom-SQL path driven by the SQL
  editor) both do
  `const time = timestampToMs(row.time); points.push({ time, value: Number(row.value) })`
  unconditionally — weaker than even the pre-existing null/`isNaN` guard being patched above.

Since `loadCustomQuery` runs arbitrary user-typed SQL, a `numerator / denominator` query with a
zero denominator reproduces the identical Y-axis-explosion bug through this route, on either
column: `timestampToMs()` (`arrow-utils.ts:22-24`) returns a numeric `time` input verbatim, so
e.g. `1.0/0.0 as time` produces a non-finite `time` with no guard, and `TimeSeriesChart`
(`XYChart.tsx:1264`) maps `time` straight to `x` with no filtering — exposing the X-axis
auto-ranging to the same corruption class as the Y-axis. This mirrors `arrow-utils.ts`, which
already treats X and Y symmetrically (`!Number.isFinite(xNum) || !Number.isFinite(yNum)` at
lines 441/613). Fix: at both sites, skip the row when
`!Number.isFinite(time) || !Number.isFinite(Number(row.value))` instead of pushing
unconditionally — the same paired X/Y `Number.isFinite` pattern used in `arrow-utils.ts`.

**Third extraction path: process-metrics page.** `ProcessMetricsPage.tsx`'s "Unified extraction
effect" (`ProcessMetricsPage.tsx:256`) has the same unguarded pattern:
`const time = timestampToMs(row.time); points.push({ time, value: Number(row.value) })`, with no
null/NaN/finiteness guard at all. This effect is fed by `activeSql`, which is set from arbitrary
user-typed SQL via a `QueryEditor`'s `onRun={handleRunQuery}`
(`ProcessMetricsPage.tsx:296-311, 378-384, 423-438`) — the same "reachable via arbitrary
user-typed SQL" condition as the perf-analysis path above. The resulting `chartData` is rendered
via `<MetricsChart data={chartData} .../>` (`ProcessMetricsPage.tsx:526`), the same `MetricsChart`
→ `TimeSeriesChart` → `XYChart.tsx` pipeline (`computeStats`/`range()` at
`XYChart.tsx:146, 726-731`) targeted above, and `/process_metrics` is a live, routed page
(`router.tsx:9,40`), not dead code. A `numerator`/`denominator` custom query run on this page
reproduces the identical Y-axis-explosion bug, on either the `value` column or (via a
`1.0/0.0 as time`-style expression) the `time` column, for the same reason given for the
perf-analysis path above. Fix: at this site too, skip the row when
`!Number.isFinite(time) || !Number.isFinite(Number(row.value))` instead of pushing
unconditionally.

## Implementation Steps

1. In `analytics-web-app/src/lib/arrow-utils.ts`, replace `isNaN(yNum)` with
   `!Number.isFinite(yNum)` at lines 424, 501, and 567.
2. In the same file, replace `isNaN(xNum) || isNaN(yNum)` with
   `!Number.isFinite(xNum) || !Number.isFinite(yNum)` at lines 441 and 613.
3. In `analytics-web-app/src/hooks/useMetricsData.ts`, in the row-extraction loop (line 93),
   compute `value = Number(row.value)` and skip the row (`continue`) when
   `!Number.isFinite(time) || !Number.isFinite(value)` (`time` is already computed via
   `timestampToMs(row.time)` on the preceding line), instead of pushing
   `{ time, value: Number(row.value) }` unconditionally.
4. In `analytics-web-app/src/routes/perf-analysis/PerformanceMetricsChart.tsx`'s
   `loadCustomQuery` (line 188), apply the same combined
   `!Number.isFinite(time) || !Number.isFinite(value)` guard before `points.push(...)`.
5. In `analytics-web-app/src/routes/ProcessMetricsPage.tsx`'s "Unified extraction effect"
   (line 256), apply the same combined `!Number.isFinite(time) || !Number.isFinite(value)`
   guard before `points.push(...)`.
6. Add test cases to `analytics-web-app/src/lib/__tests__/arrow-utils.test.ts` covering both
   `extractChartData` and `extractMultiSeriesChartData` (numeric/time path and categorical path)
   for rows containing `Infinity`/`-Infinity` in the Y column, and one covering `Infinity` in the
   X column of the numeric path — asserting the row is dropped and the remaining finite points
   are unaffected.
   - For `extractChartData`, follow the existing "should skip rows with null X/Y values" tests
     (`arrow-utils.test.ts:533`, `:552`) as the pattern for table construction and assertions.
   - For `extractMultiSeriesChartData`, there is **no existing test coverage or `describe` block**
     for this function in the file — it must be written from scratch, not adapted from a
     precedent. Its input is an array `{ table: Table; unit?: string; label?: string }[]`; build
     each `table` with the same `createMockTable(fields, rows)` helper used for `extractChartData`
     above, e.g. `extractMultiSeriesChartData([{ table: createMockTable(fields, rows) }])`. Its
     success return is `{ ok: true, xAxisMode, xColumnName, series }` where `series` is
     `ChartSeriesData[]`; assert against `result.series[i].data` (a `ChartPoint[]`, i.e. `{ x, y }`
     pairs) — not `result.data`, which is `extractChartData`'s shape, not this function's.
7. Add automated test coverage for the three guard sites from Steps 3-5:
   - Add a new `renderHook`-based test file, `analytics-web-app/src/hooks/__tests__/useMetricsData.test.ts`,
     following the mocking pattern in `useStreamQuery.test.ts` (`useStreamQuery.test.ts:1-27`,
     mocking `@/lib/arrow-stream`'s `streamQuery` and driving completion via `execute`/`act`).
     Feed a mock table with a row containing a non-finite `value`, a row containing a non-finite
     `time`, and a normal finite row; assert only the finite row survives in the returned
     `chartData`.
   - For `PerformanceMetricsChart.tsx`'s `loadCustomQuery`, extend the existing
     `analytics-web-app/src/routes/__tests__/PerformanceAnalysisPage.test.tsx`, which already
     mocks `@/lib/arrow-stream`'s `executeStreamQuery` and drives custom queries through the SQL
     editor's `onRun`, with a case whose mocked response includes a non-finite `value`/`time`
     row and asserts it is dropped (e.g. via the mocked `MetricsChart`'s `data` prop or point
     count).
   - For `ProcessMetricsPage.tsx`'s unified extraction effect, no test file or comparable mocking
     seam currently exists for this route (unlike `PerformanceAnalysisPage.test.tsx`), and adding
     one is out of scope for this plan; this site remains covered only by the manual repro in the
     Testing Strategy below.

## Files to Modify

- `analytics-web-app/src/lib/arrow-utils.ts`
- `analytics-web-app/src/lib/__tests__/arrow-utils.test.ts`
- `analytics-web-app/src/hooks/useMetricsData.ts`
- `analytics-web-app/src/hooks/__tests__/useMetricsData.test.ts` (new)
- `analytics-web-app/src/routes/perf-analysis/PerformanceMetricsChart.tsx`
- `analytics-web-app/src/routes/__tests__/PerformanceAnalysisPage.test.tsx`
- `analytics-web-app/src/routes/ProcessMetricsPage.tsx`

## Trade-offs

- **Filter at extraction vs. guard in `computeStats`**: Non-finite values could alternatively be
  filtered inside `computeStats` (`XYChart.tsx:146`) instead of at extraction. Filtering at
  extraction was chosen because it matches how `null` is already handled at the same sites,
  keeps `ChartPoint[]` a clean invariant (finite numbers only) for every downstream consumer, not
  just `computeStats`, and needs no change to `XYChart.tsx`.

## Testing Strategy

- Extend `arrow-utils.test.ts` with the cases in Implementation Step 6.
- Add the `useMetricsData.test.ts` and `PerformanceAnalysisPage.test.tsx` cases in Implementation
  Step 7, covering the `useMetricsData.ts` and `PerformanceMetricsChart.tsx` guard sites with a
  non-finite `value` row and a non-finite `time` row each.
- Run `yarn test` (and `yarn lint` / `yarn type-check`) in `analytics-web-app/`.
- Manual repro (optional): chart a SQL column with a `numerator / denominator` ratio where some
  rows have `denominator = 0`, confirm the Y-axis no longer explodes and remaining points render
  at a sensible scale.
- Manual repro for the perf-analysis path (optional, in addition to the automated test in Step 7):
  in Performance Analysis, run a custom SQL query (Steps 3-4's path) whose value or time column
  divides by zero for some rows; confirm the Y-axis no longer explodes there either.
- Manual repro for the process-metrics path (required, since Step 5's site has no automated test —
  see Step 7): on `/process_metrics`, run a custom SQL query (Step 5's path) whose value or time
  column divides by zero for some rows; confirm the Y-axis no longer explodes there either.

## Open Questions

None — the issue's root-cause analysis and suggested fix were verified directly against the
current code and match exactly.
