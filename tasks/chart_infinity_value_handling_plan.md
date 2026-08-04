# Chart Infinity Value Handling Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1424

## Overview

A single non-finite value (`Infinity`/`-Infinity`) in a chart's X or Y column — e.g. from a SQL
ratio that divides by zero for some rows — currently survives the data-extraction stage in
`arrow-utils.ts` and poisons the whole chart's auto-scaled Y-axis, collapsing every other
point/bar to a sliver near zero. This plan tightens the existing null/NaN filtering in
`arrow-utils.ts` to also drop non-finite values, matching how `null` rows are already skipped.

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

## Implementation Steps

1. In `analytics-web-app/src/lib/arrow-utils.ts`, replace `isNaN(yNum)` with
   `!Number.isFinite(yNum)` at lines 424, 501, and 567.
2. In the same file, replace `isNaN(xNum) || isNaN(yNum)` with
   `!Number.isFinite(xNum) || !Number.isFinite(yNum)` at lines 441 and 613.
3. Add test cases to `analytics-web-app/src/lib/__tests__/arrow-utils.test.ts` covering both
   `extractChartData` and `extractMultiSeriesChartData` (numeric/time path and categorical path)
   for rows containing `Infinity`/`-Infinity` in the Y column, and one covering `Infinity` in the
   X column of the numeric path — asserting the row is dropped and the remaining finite points
   are unaffected. Follow the existing "should skip rows with null X/Y values" tests
   (`arrow-utils.test.ts:533`, `:552`) as the pattern for table construction and assertions.

## Files to Modify

- `analytics-web-app/src/lib/arrow-utils.ts`
- `analytics-web-app/src/lib/__tests__/arrow-utils.test.ts`

## Trade-offs

- **Filter at extraction vs. guard in `computeStats`**: Non-finite values could alternatively be
  filtered inside `computeStats` (`XYChart.tsx:146`) instead of at extraction. Filtering at
  extraction was chosen because it matches how `null` is already handled at the same sites,
  keeps `ChartPoint[]` a clean invariant (finite numbers only) for every downstream consumer, not
  just `computeStats`, and needs no change to `XYChart.tsx`.

## Testing Strategy

- Extend `arrow-utils.test.ts` with the cases in Implementation Step 3.
- Run `yarn test` (and `yarn lint` / `yarn type-check`) in `analytics-web-app/`.
- Manual repro (optional): chart a SQL column with a `numerator / denominator` ratio where some
  rows have `denominator = 0`, confirm the Y-axis no longer explodes and remaining points render
  at a sensible scale.

## Open Questions

None — the issue's root-cause analysis and suggested fix were verified directly against the
current code and match exactly.
