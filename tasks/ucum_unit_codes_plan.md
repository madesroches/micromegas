# UCUM Unit Code Support Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1389

## Overview

Metrics arriving from CloudWatch Metric Streams (and any OTLP producer) carry
[UCUM](https://ucum.org/ucum.html) unit codes — `By/s`, `By`, `{Count}`, `1`, `MBit/s` — which the web
app's alias table does not recognize. Unrecognized units skip adaptive scaling entirely and render
as raw numbers with the code appended verbatim (`1,234,567,890 By/s` instead of `1.1 GB/s`).

This plan extends `analytics-web-app/src/lib/units.ts` to cover the full CloudWatch→OTLP unit
mapping plus the general UCUM conventions (annotations, the unity `1`), introduces canonical
*prefixed rate* units so that `kBy/s`/`MBy/s` scale correctly without a bolt-on scale-factor
mechanism, adds a canonical **dimensionless** form so `none`/`count`/`{Count}` render as bare
numbers, and normalizes the unit at `XYChart`'s Y-axis grouping site so equivalent units share one
axis.

## Current State

### The alias table

`analytics-web-app/src/lib/units.ts:7-82` is a flat `Record<string, string>` mapping alias → canonical
name. `normalizeUnit` (`units.ts:88-90`) is a single case-sensitive table lookup with passthrough:

```ts
export function normalizeUnit(unit: string): string {
  return UNIT_ALIASES[unit] ?? unit
}
```

Canonical families:

- **Time** — `nanoseconds`…`days`, recognized by `TIME_UNIT_NAMES` (`units.ts:95-103`), scaled by
  `getAdaptiveTimeUnit` in `time-units.ts`.
- **Size** — `bytes`…`terabytes` (`SIZE_UNIT_NAMES`, `units.ts:108-114`), binary factors (1 KB = 1024),
  scaled by `getAdaptiveSizeUnit` (`units.ts:196-226`).
- **Bits** — `bits`…`terabits` (`BIT_UNIT_NAMES`, `units.ts:128-134`), decimal factors (1 kbit = 1000),
  scaled by `getAdaptiveBitUnit` (`units.ts:293-320`).
- **Other** — `percent`, `degrees`, `boolean`, plus ISO 4217 currency codes detected dynamically by
  `isCurrencyUnit` (`units.ts:238-240`) against `Intl.supportedValuesOf('currency')`.

### Rate units are special-cased to the base unit only

`isSizeUnit` (`units.ts:120-123`) accepts `bytes/s` as a one-off:

```ts
return SIZE_UNIT_NAMES.has(normalized) || normalized === 'bytes/s'
```

and `getAdaptiveSizeUnit` (`units.ts:200-203`) hard-codes the same string to strip the suffix:

```ts
const isRate = normalized === 'bytes/s'
const baseUnit = (isRate ? 'bytes' : normalized) as SizeUnit
```

`isBitUnit`/`getAdaptiveBitUnit` mirror this for `bits/s`. So the only rate units that exist today
are the two base-unit rates; there is no representation for "kilobytes per second".

### Formatting

`format-value.ts:20-43` dispatches on the normalized unit (size → bit → percent → degrees → boolean →
currency) and falls through to:

```ts
return rawUnit ? `${value.toLocaleString()} ${rawUnit}` : value.toLocaleString()
```

Note the fallthrough uses **`rawUnit`**, not the normalized unit — so an alias that resolves to a
canonical name with no dedicated branch would still print its raw spelling.

### Chart axis grouping

`XYChart.tsx:237` keys the per-unit scale map on the **raw** unit string:

```ts
const u = normalizedSeries[i].unit || ''
```

Three sites must agree on that key, or a series will reference a uPlot scale that was never built:

| Site | Code | Role |
|---|---|---|
| `XYChart.tsx:237` | `normalizedSeries[i].unit \|\| ''` | builds the scale map (`scaleName = unitName \|\| 'y'`, `XYChart.tsx:261`) |
| `XYChart.tsx:760` | `s.unit \|\| 'y'` | assigns each uPlot series to a scale |
| `XYChart.tsx:381-385` | `lineUnit \|\| 'y'` | places reference lines (already guards with `scaleName in u.scales ? scaleName : 'y'`) |

Consequently `bytes`, `B`, and `By` series on one chart produce three Y axes. The same raw string is
also used for the axis label (`XYChart.tsx:735`, with a `=== 'percent'` special case) and for the
single-series axis label (`XYChart.tsx:910-911`).

### Tick rendering

`xychart-axis.ts:68-81` (`formatYAxisTick`) unconditionally appends `' ' + displayUnit`, so an empty
display unit yields a trailing space (`"100 "`).

### Other consumers of `measures.unit`

`analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx` only substitutes macros into unit
strings and passes them through; it needs no change. Tooltips (`XYChart.tsx:474-476`, `565-568`) and
header stats (`XYChart.tsx:1169-1178`) all go through `formatValueWithUnit`, which normalizes
internally.

Four more sites render `measures.unit` directly, outside `formatValueWithUnit`:

| Site | Code | Decision |
|---|---|---|
| `ProcessMetricsPage.tsx:483` | `{m.name} ({m.unit})` (measure dropdown option) | Leave raw — this is DB metadata in a discovery list, not a chart display. |
| `MeasureDiscovery.tsx:124` | `{m.name} ({m.unit})` (measure dropdown option) | Leave raw, same reasoning. |
| `ProcessMetricsPage.tsx:565` | `({selectedMeasureInfo?.unit || ''})` ("no data in range" placeholder header) | Route through `unitDisplayAbbrev(normalizeUnit(...))` **and** adopt `XYChart.tsx:1151`'s conditional rendering (`{displayUnit && <span> ({displayUnit})</span>}`) so the whole parenthetical drops for dimensionless units; otherwise a `none`/`count` measure would render a dangling `()` where today it shows `(count)`, and a `By/s` measure would still read `(By/s)` instead of `(GB/s)`. |
| `PerformanceMetricsChart.tsx:290` | `({selectedMeasureInfo?.unit || ''})` ("no data in range" placeholder header) | Same fix as above. |

## Design

### 1. Canonical prefixed rate units (replaces the "scale factor" idea)

Rather than attaching a scale factor to aliases like `kBy/s`, extend the canonical vocabulary with
`<size>/s` and `<bit>/s` for **every** prefix: `bytes/s`, `kilobytes/s`, `megabytes/s`,
`gigabytes/s`, `terabytes/s`, and likewise `bits/s`…`terabits/s`.

This gets the scale factor for free: `getAdaptiveSizeUnit` already converts the reference value to
bytes through `getSizeUnitFactor(baseUnit)`, so a `kilobytes/s` series is scaled by 1024 by the same
code path that scales a `kilobytes` series. No new mechanism, no new call sites, no risk of a caller
forgetting to apply a factor.

Generalize the two `=== 'bytes/s'` checks into a shared suffix split:

```ts
const RATE_SUFFIX = '/s'

/** Split a canonical unit into its base unit and whether it is a per-second rate. */
function splitRate(normalized: string): { base: string; isRate: boolean } {
  return normalized.endsWith(RATE_SUFFIX)
    ? { base: normalized.slice(0, -RATE_SUFFIX.length), isRate: true }
    : { base: normalized, isRate: false }
}
```

- `isSizeUnit`: `SIZE_UNIT_NAMES.has(base)` where `base` comes from `splitRate(normalizeUnit(unit))`.
- `getAdaptiveSizeUnit`: `const { base, isRate } = splitRate(normalized)`; the rest is unchanged
  (`abbrev` already appends `/s` when `isRate`).
- `isBitUnit` / `getAdaptiveBitUnit`: identical treatment against `BIT_UNIT_NAMES`.

Behavior for `bytes/s` and `bits/s` is unchanged; the existing tests keep passing.

### 2. Canonical dimensionless unit

Canonical form for a dimensionless quantity is the **empty string** `''`, which every existing site
already treats as "no unit": `format-value.ts:42` returns a bare `toLocaleString()`, and
`XYChart`'s scale key falls back to `'y'`. Introducing a distinct sentinel (`'dimensionless'`) would
require unwrapping it at every display site, so `''` is the cheaper canonical form.

Aliases mapping to `''`: `{Count}` (via the annotation rule below), `1`, `none`, `None`, `count`,
`Count`, `counts`, `units`, `unit`, `iterations`. A dimensionless **rate** canonicalizes to `'/s'`
(from `{Count}/s`, `1/s`, `count/s`).

`format-value.ts` fallthrough changes from `rawUnit` to the normalized unit — but the normalized
(spelled-out) name is not what should be *shown*: `centimeters` should still read `cm`, matching
today's `42 cm` from the raw-unit fallthrough. So `units.ts` gets a small canonical→display-abbreviation
map, and the fallthrough goes through it instead of printing the canonical name directly:

```ts
// units.ts — declared after SIZE_UNITS and BIT_UNITS (§1) since it derives from them.
const CANONICAL_DISPLAY_ABBREV: Record<string, string> = {
  '': '',
  'percent': '%',
  'degrees': '°',
  'celsius': '°C',
  'centimeters': 'cm',
  // Time — hand-listed rather than imported from time-units.ts's TIME_UNITS, which would create an
  // import cycle (time-units.ts already imports normalizeUnit from this module).
  'nanoseconds': 'ns',
  'microseconds': 'µs',
  'milliseconds': 'ms',
  'seconds': 's',
  'minutes': 'min',
  'hours': 'h',
  'days': 'd',
  // Size/bit — derived from SIZE_UNITS/BIT_UNITS (§1) so the abbreviation can never drift from the
  // adaptive-scaling tables; each also gets its `/s` rate form, matching getAdaptiveSizeUnit's/
  // getAdaptiveBitUnit's own `bestUnit.abbrev + '/s'` convention.
  ...Object.fromEntries(SIZE_UNITS.flatMap((u) => [[u.unit, u.abbrev], [`${u.unit}/s`, `${u.abbrev}/s`]])),
  ...Object.fromEntries(BIT_UNITS.flatMap((u) => [[u.unit, u.abbrev], [`${u.unit}/s`, `${u.abbrev}/s`]])),
}

/** The short form shown to users for a canonical unit; falls back to the canonical name itself. */
export function unitDisplayAbbrev(canonicalUnit: string): string {
  return CANONICAL_DISPLAY_ABBREV[canonicalUnit] ?? canonicalUnit
}
```

```ts
// format-value.ts fallthrough
const displayUnit = unitDisplayAbbrev(unit)
if (!displayUnit) return value.toLocaleString()
if (displayUnit.startsWith('/')) return `${value.toLocaleString()}${displayUnit}`
return `${value.toLocaleString()} ${displayUnit}`
```

Unknown units still pass through verbatim (`unitDisplayAbbrev` falls back to its input when there is
no map entry, and `normalizeUnit` itself falls back to the raw string when there is no alias), so
`formatValueWithUnit(42, 'widgets') === '42 widgets'` is preserved.

**Not in scope:** compact SI notation for large dimensionless counters (`1.2G` rather than
`1,234,567,890`). That would change every unitless chart in the app, well beyond this issue — see
Trade-offs.

### 3. UCUM annotation handling in `normalizeUnit`

UCUM `{...}` is an annotation on an otherwise dimensionless (or already-typed) quantity. Handle it as
a **rule**, not as table entries, so `{request}`, `{packet}`, `{error}`, `{fault}`, `{operation}`,
`{thread}`, `{connection}` — and any annotation nobody has thought of yet — all work without a table
edit (open/closed):

```ts
const ANNOTATION_RE = /\{[^}]*\}/g

export function normalizeUnit(unit: string): string {
  const direct = UNIT_ALIASES[unit]
  if (direct !== undefined) return direct
  if (!unit.includes('{')) return unit
  const stripped = unit.replace(ANNOTATION_RE, '')
  return UNIT_ALIASES[stripped] ?? stripped
}
```

- `{Count}` → `''` (dimensionless) — no special case needed for CloudWatch's Count.
- `{request}/s` → `'/s'` (dimensionless rate).
- `By{net}` → `'By'` → table → `'bytes'` (the post-strip lookup covers annotated real units).
- Table lookup runs **first**, so an explicit entry can always override the rule.

The lookup stays **case-sensitive**: UCUM `B` is the bel and `By` is the byte, and the existing table
already relies on case to separate `B`(ytes, pragmatic legacy alias) from `bit`. Lowercasing keys
would collapse distinct units.

### 4. Alias table additions

The CloudWatch→OTLP table is regular: every size/bit code has a bare form and a `/s` form that map to
the same canonical base. Encode that once and expand it, rather than writing 40 hand-maintained
entries where a copy-paste slip is invisible:

```ts
/**
 * UCUM/OTLP codes for scalable units → canonical base name. Each also implies `<code>/s`.
 * Includes the byte/bit spellings already present as bare-form entries in the hand-written
 * table above (`B`, `KB`, `kb`, `MB`, `GB`, `TB`, `bit`, `kbit`, `Mbit`, `Gbit`, `Tbit`, and the
 * spelled-out canonical names) so their `/s` forms are generated here instead of hand-writing a
 * matching rate entry for each one. Re-listing a bare form is harmless — both definitions map to
 * the same canonical name.
 */
const UCUM_SCALED_CODES: Record<string, string> = {
  // Bytes — decimal and binary prefixes both map to the app's 1024-based canonical units.
  'By': 'bytes', 'B': 'bytes', 'bytes': 'bytes',
  'kBy': 'kilobytes', 'KiBy': 'kilobytes', 'KB': 'kilobytes', 'kb': 'kilobytes', 'kilobytes': 'kilobytes',
  'MBy': 'megabytes', 'MiBy': 'megabytes', 'MB': 'megabytes', 'megabytes': 'megabytes',
  'GBy': 'gigabytes', 'GiBy': 'gigabytes', 'GB': 'gigabytes', 'gigabytes': 'gigabytes',
  'TBy': 'terabytes', 'TiBy': 'terabytes', 'TB': 'terabytes', 'terabytes': 'terabytes',
  // Bits — AWS spells megabits/gigabits `MBit`/`GBit` but kilobits/terabits `kbit`/`Tbit`; accept both cases.
  'bit': 'bits', 'bits': 'bits',
  'kBit': 'kilobits', 'kbit': 'kilobits', 'kilobits': 'kilobits',
  'MBit': 'megabits', 'Mbit': 'megabits', 'megabits': 'megabits',
  'GBit': 'gigabits', 'Gbit': 'gigabits', 'gigabits': 'gigabits',
  'TBit': 'terabits', 'Tbit': 'terabits', 'terabits': 'terabits',
}

function expandRates(codes: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {}
  for (const [code, canonical] of Object.entries(codes)) {
    out[code] = canonical
    out[`${code}/s`] = `${canonical}/s`
  }
  return out
}
```

`UNIT_ALIASES` becomes a spread of the hand-written entries plus `expandRates(UCUM_SCALED_CODES)`,
keeping its exported type `Record<string, string>` (the existing test at `units.test.ts:148-155`
indexes it directly and keeps working). This generates every required `/s` form in one place —
`B/s`, `KB/s`, `MB/s`, `GB/s`, `TB/s`, `bit/s`, `kbit/s`, `Mbit/s`, `Gbit/s`, `Tbit/s` — without a
second source map or hand-written rate entries.

Additional flat entries:

| Alias | Canonical | Source |
|---|---|---|
| `1`, `none`, `None`, `count`, `Count`, `counts`, `units`, `unit`, `iterations` | `''` | UCUM unity, Unreal, Rust `imetric!` |
| `1/s`, `count/s` | `'/s'` | dimensionless rate |
| `Cel`, `celsius` | `celsius` | OTel semconv |
| `cm`, `centimeters` | `centimeters` | Unreal |

`celsius` gets a formatting branch in `format-value.ts` (`value.toFixed(1) + '°C'`), mirroring
`degrees`. `centimeters` is a passthrough canonical name — no adaptive m/km ladder is added (see
Trade-offs). Axis and chart-title labels for both units go through `unitDisplayAbbrev` (§5), so they
read `cm` / `°C` rather than the spelled-out canonical name.

**Currency-collision check** (`units.ts:238-240` uppercases before matching ISO 4217): the new
three-character keys are `kBy`, `MBy`, `GBy`, `TBy`, `Cel` → `KBY`, `MBY`, `GBY`, `TBY`, `CEL`; none
is an ISO 4217 code. Note the check runs on the *normalized* unit, so canonical names (`bytes`,
`celsius`, `''`) are what actually reach it — all longer than three characters or empty. A test will
assert no key in `UNIT_ALIASES` normalizes to something `isCurrencyUnit` accepts.

`Hz`, `W`, `J` need **no** entries: they are already rendered correctly by passthrough
(`1,234 Hz`), and adding identity aliases would be noise.

`rate` also needs no entry: its only call site in the repo is a Rust test fixture
(`rust/analytics/tests/metrics_test.rs:205`, `fmetric!("tagged_float", "rate", 2.5)`), not a
production metric, so it is deliberately left unmapped (passthrough, renders `1,234 rate`).

### 5. Normalize the axis grouping key in `XYChart`

Add one exported helper to `units.ts` and use it at every site that derives a scale key, so the three
sites cannot drift apart:

```ts
/** Scale-grouping key for a series unit: canonical form, `''` when dimensionless/absent. */
export function unitScaleKey(unit: string | undefined | null): string {
  return normalizeUnit(unit ?? '')
}
```

Changes in `XYChart.tsx`:

- `:237` → `const u = unitScaleKey(normalizedSeries[i].unit)`
- `:760` → `const scaleName = unitScaleKey(s.unit) || 'y'`
- `:381-385` → `const scaleName = unitScaleKey(lineUnit) || 'y'` (the existing
  `scaleName in u.scales` guard stays)
- `:735` axis label → `adaptiveInfo?.abbrev ?? unitDisplayAbbrev(scaleInfo.unitName)` (`unitName` is
  already canonical). Only the fallback changes — replacing the ad-hoc `=== 'percent'` special case
  with the shared map so `centimeters`/`celsius` render as `cm`/`°C` too — while the `adaptiveInfo?.abbrev`
  prefix that produces `GB/s`/`ms`/`Mbit` is unchanged.
- `:909` single-series label → `adaptiveTimeUnit?.abbrev ?? adaptiveSizeUnit?.abbrev ??
  adaptiveBitUnit?.abbrev ?? unitDisplayAbbrev(normalizeUnit(primaryUnit))`. Same fallback-only
  replacement for the ad-hoc `=== 'percent'` check; the three adaptive-abbrev fallbacks stay in place.
- `:292` header `displayUnit` → fall back to `unitDisplayAbbrev(normalizeUnit(primaryUnit))` so a
  `none`/`count` series shows no suffix and `cm`/`celsius` series show their short form.

`seriesInfoForTooltip` (`:827`) keeps the **raw** unit — `formatValueWithUnit` normalizes internally,
and there is no cross-series key involved.

### 6. Empty display unit in tick labels

`formatYAxisTick` (`xychart-axis.ts:68-81`) appends `' ' + displayUnit` on all five branches. With a
dimensionless axis this leaves a trailing space. Build the suffix once:

```ts
const suffix = displayUnit ? (displayUnit.startsWith('/') ? displayUnit : ' ' + displayUnit) : ''
```

and append `suffix` in each branch.

## Implementation Steps

1. **`analytics-web-app/src/lib/units.ts`**
   - Add `UCUM_SCALED_CODES` + `expandRates`, spread into `UNIT_ALIASES`.
   - Add the flat dimensionless / `celsius` / `cm` entries.
   - Rewrite `normalizeUnit` with the annotation-stripping rule.
   - Add `splitRate`; generalize `isSizeUnit`, `getAdaptiveSizeUnit`, `isBitUnit`,
     `getAdaptiveBitUnit` to any `<canonical>/s`.
   - Add `CANONICAL_DISPLAY_ABBREV` and export `unitScaleKey` and `unitDisplayAbbrev`.
2. **`analytics-web-app/src/lib/format-value.ts`**
   - Add the `celsius` branch.
   - Switch the fallthrough from `rawUnit` to `unitDisplayAbbrev(normalized unit)`; bare number when
     empty; no space before a leading-slash unit.
3. **`analytics-web-app/src/components/xychart-axis.ts`**
   - Compute the unit suffix once in `formatYAxisTick`; handle empty and `/s`-leading units.
4. **`analytics-web-app/src/components/XYChart.tsx`**
   - Route the three scale-key sites through `unitScaleKey`.
   - Route the axis/header label **fallbacks** (`:292`, `:735`, `:909`) through `unitDisplayAbbrev`,
     replacing the ad-hoc `=== 'percent'` checks, while keeping each site's `adaptiveInfo?.abbrev` /
     `adaptiveTimeUnit?.abbrev ?? adaptiveSizeUnit?.abbrev ?? adaptiveBitUnit?.abbrev` prefix intact.
5. **`analytics-web-app/src/routes/ProcessMetricsPage.tsx`** and
   **`analytics-web-app/src/routes/perf-analysis/PerformanceMetricsChart.tsx`**
   - Route the "no data in range" placeholder header's unit (`:565` / `:290`) through
     `unitDisplayAbbrev(normalizeUnit(...))`, and wrap the parenthetical in the same conditional
     `displayUnit &&` guard as `XYChart.tsx:1151` so an empty abbreviation drops the whole
     `(...)` instead of rendering `()`, matching `XYChart`'s loaded-chart header.
6. **Tests** — extend `units.test.ts`, `format-value.test.ts`, and add coverage for
   `formatYAxisTick` and the axis grouping (see Testing Strategy). Update the two existing
   assertions that pin the old behavior: `units.test.ts:123-124`
   (`normalizeUnit('none') === 'none'`, `normalizeUnit('count') === 'count'`).

## Files to Modify

- `analytics-web-app/src/lib/units.ts`
- `analytics-web-app/src/lib/format-value.ts`
- `analytics-web-app/src/components/xychart-axis.ts`
- `analytics-web-app/src/components/XYChart.tsx`
- `analytics-web-app/src/routes/ProcessMetricsPage.tsx`
- `analytics-web-app/src/routes/perf-analysis/PerformanceMetricsChart.tsx`
- `analytics-web-app/src/lib/__tests__/units.test.ts`
- `analytics-web-app/src/lib/__tests__/format-value.test.ts`
- `analytics-web-app/src/components/__tests__/xychart-axis.test.ts` (extend, or create if absent)
- `mkdocs/docs/web-app/notebooks/variables.md`
- `mkdocs/docs/query-guide/schema-reference.md`

## Trade-offs

**Canonical `<prefix>/s` units vs. per-alias scale factors.** The issue suggests attaching a scale
factor to `kBy/s`. That would require every consumer of `normalizeUnit` to also fetch and apply a
factor — a second return value the existing call sites silently ignore, i.e. a bug waiting to happen.
Extending the canonical vocabulary instead reuses the size/bit factor ladders that already exist and
touches only the two `=== 'bytes/s'` predicates.

**Decimal UCUM prefixes mapped onto binary canonical units.** UCUM `kBy` is exactly 1000 bytes while
the app's `kilobytes` is 1024; `KiBy` is exactly 1024. Mapping both to `kilobytes` introduces up to
7.4% error at the giga level for strictly-decimal producers. This matches what the issue asks for
(CloudWatch's Kilobytes are 1024-based), and matches the app's existing convention that `KB` means
1024 bytes. Splitting the ladder into decimal and binary families would double the canonical unit set
and the axis-grouping cardinality for a difference nobody reads off a chart axis.

**Dimensionless drops the label.** Mapping `count` → `''` means a counter series renders `1,234,567`
rather than `1,234,567 count`, and its axis carries no unit label. The series *name* still carries the
meaning, and this is what makes `5 none` render as `5`. The alternative — keeping the suffix for
`count` while dropping it for `none` — would split the dimensionless family on cosmetics and leave
`{Count}` and `count` on separate Y axes.

**No compact SI notation.** The issue notes that a large counter renders as `1,234,567,890`.
`toLocaleString()` at least groups thousands; switching to `1.2G` would change every unitless value
in the app (tooltips, stat headers, table cells) and deserves its own issue.

**No cm→m→km ladder.** The issue mentions `cm` passes through unscaled. Adding a length ladder means
a fourth adaptive family (`LENGTH_UNIT_NAMES`, `getAdaptiveLengthUnit`) for one Unreal call-site
family; deferred as out of scope. `cm` is canonicalized so a later ladder is a drop-in.

## Testing Strategy

Vitest, run from `analytics-web-app/` with `yarn test`; `yarn lint` and `yarn type-check` before
commit.

**`units.test.ts`**
- Every CloudWatch OTLP code from the issue's table normalizes to the expected canonical name — a
  table-driven test over all 27 pairs, so the mapping is checked as a whole rather than sampled.
- Both bit spellings: `MBit`/`Mbit` → `megabits`, `GBit`/`Gbit`, `TBit`/`Tbit`, `kBit`/`kbit`.
- Case sensitivity preserved: `normalizeUnit('B')` → `bytes` and `normalizeUnit('By')` → `bytes`
  (distinct keys, same target), and no lowercasing is introduced.
- Annotations: `{Count}`, `{request}`, `{connection}` → `''`; `{request}/s` → `'/s'`;
  `By{net}` → `'bytes'`; `1` → `''`.
- `isSizeUnit`/`isBitUnit` accept every prefixed rate (`kBy/s`, `MBy/s`, `GBit/s`, …) and still
  reject `percent` and dimensionless.
- `getAdaptiveSizeUnit(1, 'MBy/s')` → conversion into `MB/s` with the 1024² factor applied; the
  issue's headline case `getAdaptiveSizeUnit(1_234_567_890, 'By/s')` → `GB/s`.
- No alias normalizes to a value `isCurrencyUnit` accepts.
- Update `units.test.ts:123-124` (`none`, `count` now normalize to `''`).
- `unitDisplayAbbrev` maps `percent`→`%`, `degrees`→`°`, `celsius`→`°C`, `centimeters`→`cm`, `''`→`''`,
  the time/size/bit canonical names to their short forms (`milliseconds`→`ms`, `kilobytes`→`KB`,
  `megabits`→`Mbit`, …) and their `/s` forms (`bytes/s`→`B/s`, `kilobits/s`→`kbit/s`, …), and passes
  through any other, genuinely unmapped canonical name unchanged (e.g. `widgets`→`widgets`).
- Regression guard for the all-zero-series case: for every name in `TIME_UNIT_NAMES` (bare forms
  only — there is no canonical time rate unit), and for every name in `SIZE_UNIT_NAMES` and
  `BIT_UNIT_NAMES` (bare and `/s` forms), `unitDisplayAbbrev(name) !== name` — i.e. an adaptive
  family never falls through to its spelled-out canonical name.

**`format-value.test.ts`**
- `formatValueWithUnit(1234567890, 'By/s')` → `'1.1 GB/s'` (the issue's reported symptom).
- `formatValueWithUnit(5, 'none')` → `'5'`; `(1234567, 'count')` → grouped bare number;
  `(1234, '{Count}/s')` → `'1,234/s'`.
- `formatValueWithUnit(21.5, 'Cel')` → `'21.5°C'`.
- `formatValueWithUnit(42, 'cm')` → `'42 cm'` — regression guard that the new `centimeters` canonical
  name still displays as `cm`, not the spelled-out name, via the fallthrough's `unitDisplayAbbrev`.
- Unknown units still append verbatim (`'42 widgets'`) — regression guard on the `rawUnit` →
  normalized-unit switch.

**`xychart-axis.test.ts`**
- `formatYAxisTick` with an empty display unit produces no trailing space (`'100'`, `'0'`).
- With a `/s` display unit produces `'100/s'`.
- Currency and existing numeric branches unchanged.

**`XYChart` axis grouping**
- No `XYChart.tsx` component test exists today (`src/components/__tests__/` has no `uplot` mock), so
  standing one up is out of scope here. A direct unit test in `units.test.ts` asserting
  `unitScaleKey('bytes') === unitScaleKey('B') === unitScaleKey('By')` covers the grouping contract,
  backed by the manual verification below for the end-to-end chart behavior.

**Manual verification**
Start the monolith (`python3 local_test_env/ai_scripts/start_services.py --monolith`), open a chart
on a CloudWatch-sourced measure such as `amazonaws.com/AWS/RDS/NetworkThroughput_max` (`By/s`), and
confirm the axis, tooltip, and stat header read `GB/s` rather than raw `By/s` values. Also select a
dimensionless (`none`/`count`) measure with no data in the current range on `ProcessMetricsPage` and
`PerformanceMetricsChart` and confirm the placeholder header title has no trailing `()`.

## Documentation

Two mkdocs pages enumerate the unit vocabulary and need updating (precedent:
`tasks/completed/1326_money_currency_format_plan.md` touched both alongside the same four source
files):

- `mkdocs/docs/web-app/notebooks/variables.md:144` lists example units for `format_value` — add the
  UCUM/OTLP codes (`By`, `MBy/s`, …) and the new dimensionless (`1`, `{Count}`, `none`) additions to
  the enumerated vocabulary.
- `mkdocs/docs/query-guide/schema-reference.md:280` documents the `measures.unit` column and
  currently calls out only the currency case — add a note that CloudWatch/OTLP UCUM codes and
  dimensionless units (`1`, `{Count}`) are also normalized and rendered adaptively.

The in-file doc comment at the top of `units.ts` should also be extended to state the two invariants
a future editor could easily break: the lookup is **case-sensitive** (UCUM `B` ≠ `By`), and canonical
rate units are exactly `<canonical base>/s`.

## Open Questions

- **Dropping the `count` suffix** is the visible behavior change for existing internal dashboards
  (51 `imetric!` call sites). The Trade-offs section argues for it; flagging it in case the loss of
  the axis label is unwanted.
