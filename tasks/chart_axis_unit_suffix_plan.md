# Suppress Y-Axis Unit Suffix for Opaque Units on Single-Line Charts Plan

## Overview

Looking at https://github.com/madesroches/micromegas/issues/1503 (y-axis tick
labels getting truncated on the left edge, e.g. `"100 ops_per_sec"` rendering
as `"0 ops_per_sec"` repeated) prompted a related but distinct UX question:
**should a chart repeat a long, opaque unit string on every single tick at
all?** This plan addresses that question. It is *not* a fix for #1503 — the
actual sizing/truncation bug (the axis not reserving enough left-edge space
for its label text) remains open and will be fixed separately later. This
change is a complementary simplification that happens to reduce how often the
truncation bug is visible, since it removes the unit text from tick labels in
the specific case described below — but it doesn't touch axis sizing/layout
at all.

The idea: for a single-line chart, when the unit is "opaque" (not one of the
codebase's recognized units that already map to a short symbol/abbreviation),
drop the unit suffix from the y-axis ticks entirely. The unit is not lost —
it's still shown in the chart header title, the min/p99/max/avg stats row,
and the tooltip, all of which are single-line-only UI elements independent of
the axis formatter. Multi-line charts are unaffected, since a shared axis is
the *only* place a plain unit is surfaced there today.

## Current State

### Axis tick formatting — `analytics-web-app/src/components/xychart-axis.ts`

`formatYAxisTick` (`xychart-axis.ts:154-169`) appends a unit suffix to every
tick via `unitSuffix(displayUnit)` (`units.ts:450-453`), except when
`currencyCode` is set (routes through `formatCurrencyValue` instead). It
already treats an empty `displayUnit` as "no suffix" — `unitSuffix('')`
returns `''`, and this is covered by an existing test
(`xychart-axis.test.ts:178-179`). So no change is needed inside
`formatYAxisTick` itself — suppressing the suffix is just a matter of passing
`''` as `displayUnit` from the call site when appropriate.

Called from two places in `XYChart.tsx`:
- Multi-line axes (one per unit-scale group), inside the
  `isMultiSeries && normalizedSeries.length > 1` branch: `XYChart.tsx:743-757`.
- Single-line axis, inside the `else` branch (covers both the legacy
  `data`/`unit` props and a `series` prop with exactly one entry):
  `XYChart.tsx:999-1013`.

`normalizedSeries.length === 1` (reachable only via the single-line `else`
branch, `XYChart.tsx:893`) is the existing, reliable "exactly one line" test —
no new series-counting logic is needed.

### Known vs. opaque units — `analytics-web-app/src/lib/units.ts`

- Three adaptively-scaled ("smart") families each get a short abbreviation
  that changes with magnitude: time (`isTimeUnit`/`getAdaptiveTimeUnit`,
  `time-units.ts`), size (`isSizeUnit`/`getAdaptiveSizeUnit`, `units.ts:210,287`),
  bits (`isBitUnit`/`getAdaptiveBitUnit`, `units.ts:231,384`).
- Currency is its own formatter (`isCurrencyUnit`/`formatCurrencyValue`,
  `units.ts:329,336`), already bypassing `unitSuffix` entirely in
  `formatYAxisTick`.
- A fixed lookup table, `CANONICAL_DISPLAY_ABBREV` (`units.ts:414-434`), maps
  every other *known* canonical unit to a short symbol: `percent → %`,
  `degrees → °`, `celsius → °C`, `centimeters → cm`, dimensionless → `''`,
  plus every time/size/bit canonical name (kept in sync with the adaptive
  tables, so a time/size/bit unit still gets a short abbreviation even when
  adaptive scaling doesn't kick in, e.g. `stats.p99 === 0`).
- `unitDisplayAbbrev(canonicalUnit)` (`units.ts:437-439`) looks up that table
  and **falls back to returning the input unchanged** when there's no entry —
  this fallback is exactly the "opaque unit" case: an arbitrary,
  unbounded-length string (`ops_per_sec`, `requests`, ...) gets concatenated
  onto every tick value with no abbreviation.
- Table membership isn't the whole "safe to repeat on every tick" story,
  though. `unitSuffix` (`units.ts:441-453`) already treats any
  symbol-*prefixed* display unit — matching `/^[/°%]/`, i.e. a leading `/`,
  `°`, or `%` — as attaching directly to the value with no leading space,
  and its docstring explicitly calls this out as covering out-of-vocabulary
  units like `°F` or `%CPU`, not just table entries. The bare rate form `/s`
  is one such case that's already reachable today: `'1/s'` and `'count/s'`
  are existing aliases (`units.ts:104-105`, asserted in
  `units.test.ts:146-147`) that `normalizeUnit` maps to `/s`, but
  `CANONICAL_DISPLAY_ABBREV` only has unit-specific `<size|bit>/s` entries
  (e.g. `bytes/s`, `kilobits/s`), not a bare `/s` entry — so `/s` is not a
  table member even though it's short, symbol-form, and already
  rendered/tested compactly by `formatYAxisTick`/`unitSuffix`
  (`xychart-axis.test.ts`, "attaches a /s display unit without a leading
  space").

There's no existing predicate for "is this canonical unit one we actually
know how to abbreviate, or otherwise safe to repeat on every tick" —
`CANONICAL_DISPLAY_ABBREV` is only ever read via `unitDisplayAbbrev`'s
fallback-to-self lookup, which conflates "known, and it happens to abbreviate
to itself" (impossible today — every entry maps to a non-identity symbol)
with "unknown." A plain membership check on `CANONICAL_DISPLAY_ABBREV` is a
good start but, per the `/s` case above, must also recognize `unitSuffix`'s
symbol-prefixed forms so it doesn't newly suppress a suffix that's already
short and already safe.

### Where else the unit is shown for a single-line chart (`XYChart.tsx`)

- Header title: `{displayTitle}{displayUnit && <span ...> ({displayUnit})</span>}` — `XYChart.tsx:1158`.
- Stats row (min/p99/max/avg), rendered only when `!showMultiSeriesHeader`:
  `XYChart.tsx:1176-1186`, via `formatValueWithUnit` (`format-value.ts:53-58`).
- Tooltip (single-series plugin): `XYChart.tsx:567-570`.

All three read `displayUnit`/`primaryUnit` independently of the y-axis tick
formatter, so removing the tick suffix loses no information for a single-line
chart.

**Multi-line caveat (unchanged by this plan):** uPlot axis `label`/title text
is never set anywhere in `XYChart.tsx`, and the multi-line header/legend
doesn't show per-axis units either — so for a multi-line chart, the tick
suffix is currently the *only* static indicator of what a shared axis means.
That's why this fix is scoped to the single-line path only; fixing the
multi-line case (e.g. via a real axis title) is a separate concern, out of
scope here.

## Design

Add a small predicate to `units.ts` and use it, plus the existing
single-line/multi-line branch split, to decide whether to pass the real unit
or `''` into `formatYAxisTick` at the single-line call site.

```ts
// units.ts, near unitDisplayAbbrev

/**
 * Longest symbol-prefixed unit this codebase actually cites as
 * short-and-safe: `%CPU` (4 chars). Bounds the `isKnownUnit` carve-out below
 * so a long opaque string that merely starts with `/`, `°`, or `%` (e.g. a
 * hypothetical `%utilization_ratio`) isn't misclassified as safe to repeat.
 */
const MAX_SYMBOL_PREFIX_UNIT_LENGTH = 4

/**
 * True when `canonicalUnit` is safe to repeat on every y-axis tick without
 * risking overflow: either it has a known short abbreviation (time/size/bit/
 * percent/degrees/celsius/centimeters/dimensionless, i.e. `unitDisplayAbbrev`
 * would not just echo the input back unchanged), or it's already short *and*
 * symbol-prefixed — leading `/`, `°`, or `%`, and no longer than
 * `MAX_SYMBOL_PREFIX_UNIT_LENGTH` — of the kind `unitSuffix` renders
 * compactly (e.g. the bare rate unit `/s`, or an out-of-vocabulary `°F`/
 * `%CPU`). The prefix test mirrors `unitSuffix`'s own regex, but adds a
 * length bound: `unitSuffix`'s test only decides spacing and says nothing
 * about length, so used alone it would also match a long opaque unit that
 * happens to start with one of these symbols (e.g. `%utilization_ratio`).
 */
export function isKnownUnit(canonicalUnit: string): boolean {
  if (canonicalUnit in CANONICAL_DISPLAY_ABBREV) return true
  return (
    /^[/°%]/.test(canonicalUnit) && canonicalUnit.length <= MAX_SYMBOL_PREFIX_UNIT_LENGTH
  )
}
```

`isKnownUnit` is checked against the *normalized* unit, so it works whether
or not adaptive scaling actually applied (mirrors how `yAxisUnit` itself is
derived at `XYChart.tsx:915`). Currency is handled separately, since it
already has its own `isCurrencyScale` flag at the call site.

In `XYChart.tsx`'s single-line branch, at the axis `values` closure
(`XYChart.tsx:1007-1011`), suppress the suffix when the unit isn't known and
isn't currency:

```ts
const isKnownAxisUnit = isCurrencyScale || isKnownUnit(normalizeUnit(primaryUnit))
...
values: (_u: uPlot, vals: number[]) => {
  return vals.map((v) =>
    formatYAxisTick(v, 1, isKnownAxisUnit ? yAxisUnit : '', isCurrencyScale ? primaryUnit : null)
  )
},
```

`yAxisUnit` itself (`XYChart.tsx:915`) is untouched — it's still passed to the
header title, stats row, and tooltip exactly as today. Only the value handed
to `formatYAxisTick` changes.

No change is made to the multi-line branch (`XYChart.tsx:743-757`) — every
unit-scale group there always has ≥1 line, but the group can still be shared
by 2+ series with an opaque unit (`normalizedSeries.length > 1` overall), and
per the caveat above there's currently no other place that unit is shown, so
suppressing it there would remove the only context. That branch is left as
today's behavior.

## Implementation Steps

1. `analytics-web-app/src/lib/units.ts`: add `isKnownUnit(canonicalUnit: string): boolean`
   next to `unitDisplayAbbrev`, checking membership in `CANONICAL_DISPLAY_ABBREV`
   *or* a leading `/`, `°`, or `%` combined with a `MAX_SYMBOL_PREFIX_UNIT_LENGTH`
   (4, the length of the longest cited case, `%CPU`) length bound — mirroring
   `unitSuffix`'s symbol-prefix test but adding the length check it lacks — so
   already-compact forms like `/s`, `°F`, `%CPU` count as known while a long
   opaque unit that merely starts with one of those symbols does not.
2. `analytics-web-app/src/components/XYChart.tsx`: in the single-line branch,
   compute `isKnownAxisUnit` (currency OR `isKnownUnit(normalizeUnit(primaryUnit))`)
   next to the existing `yAxisUnit`/`isCurrencyScale` computation
   (`XYChart.tsx:915-916`), and use it to pass `''` instead of `yAxisUnit` into
   `formatYAxisTick` at the axis `values` callback (`XYChart.tsx:1007-1011`).
3. Add unit tests (see Testing Strategy).
4. Manually verify against a chart with an opaque unit (e.g. `redis_ops_per_sec
   | ops_per_sec`, the measure named in #1503) and a couple of "known unit"
   single-line charts (e.g. a `milliseconds` or `bytes` measure, and a
   `percent` measure) to confirm ticks are unaffected for those.

## Files to Modify

- `analytics-web-app/src/lib/units.ts` — add `isKnownUnit`.
- `analytics-web-app/src/components/XYChart.tsx` — gate the single-line axis
  `values` callback's unit argument on `isKnownAxisUnit`.
- `analytics-web-app/src/lib/__tests__/units.test.ts` — tests for `isKnownUnit`.
- `analytics-web-app/src/components/__tests__/xychart-axis.test.ts` — no
  change needed (`formatYAxisTick` itself is untouched); if an `XYChart`
  component test harness exists that already asserts single-line tick text,
  extend it, otherwise this is covered by the `isKnownUnit` unit test plus
  manual verification.

## Trade-offs

- **Scoped to single-line charts only, not "any axis with a plain unit."**
  A multi-line chart sharing an opaque-unit axis still has the same
  truncation risk today. Fixing that would need a different mechanism (e.g.
  an actual uPlot axis title) since there's no other place multi-line units
  are shown — left out of scope to keep this change minimal and low-risk.
- **`isKnownUnit` as table membership (+ a length-bounded symbol-prefix test)
  vs. a length heuristic alone.** Checking `CANONICAL_DISPLAY_ABBREV`
  membership reuses the existing single source of truth for "units we know
  how to abbreviate," which is exact. The symbol-prefix carve-out on top of
  it is *not* exact on its own — `unitSuffix`'s `/^[/°%]/` regex only decides
  leading-space formatting and says nothing about length, so reused bare it
  would also match a long opaque unit that happens to start with `/`, `°`, or
  `%` (e.g. a hypothetical `%utilization_ratio`). Pairing that regex with a
  `MAX_SYMBOL_PREFIX_UNIT_LENGTH` bound (4, sized to the longest cited case,
  `%CPU`) closes that gap: it still catches the short, symbol-form units
  (`/s`, `°F`, `%CPU`) table membership alone would miss, while excluding
  long opaque strings. This is a length heuristic for that one carve-out, not
  a rejection of length heuristics in general — the difference from "guessing
  from string length" as a global test is that it's scoped narrowly (only
  applies to strings that already pass the symbol-prefix test) and bounded by
  a value tied to a real cited case rather than an arbitrary threshold.
- **Suffix suppressed entirely vs. truncated/ellipsized.** Dropping it is
  simpler than adding text-truncation/rotation logic to the axis, and loses
  no information since the unit remains visible in the header/stats/tooltip
  for single-line charts.

## Testing Strategy

- Unit test `isKnownUnit` in `units.test.ts`: true for `'milliseconds'`,
  `'bytes'`, `'percent'`, `'degrees'`, `'celsius'`, `'centimeters'`, `''`
  (dimensionless), and for the symbol-prefixed cases `'/s'`, `'°F'`, `'%CPU'`;
  false for an arbitrary unrecognized unit like `'ops_per_sec'` or
  `'requests'`, and false for a long unit that merely starts with a
  symbol-prefix character, e.g. `'%utilization_ratio'` or
  `'/requests_dropped'` — this is the case the length bound on the
  symbol-prefix carve-out exists to exclude.
- Manual: build a `redis_ops_per_sec | ops_per_sec`-style chart and confirm
  y-axis ticks now render as plain numbers (`100`, `200`, `300`), while the
  header title/stats/tooltip still show `ops_per_sec`. Spot-check a
  single-line chart on a known unit (e.g. milliseconds, bytes, percent) to
  confirm those ticks are unchanged. Spot check a multi-line chart with an
  opaque unit to confirm its ticks still show the suffix (unchanged
  behavior). Separately, #1503's own truncation repro (long unit text getting
  clipped against the left edge) is out of scope here and should still be
  re-verified once that bug is fixed on its own track.
- Run `yarn lint`, `yarn type-check`, `yarn test` in `analytics-web-app/`.

## Open Questions

None — scope confirmed with the user: suppress the axis-tick unit suffix only
when the unit is not one of the codebase's recognized/abbreviated units *and*
the chart has exactly one line; leave multi-line charts as-is. This is
independent of #1503's axis-sizing/truncation bug, which stays open and will
be fixed on its own.
