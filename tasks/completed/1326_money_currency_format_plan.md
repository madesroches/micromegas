# Issue #1326: Money/Currency Format Support Plan

GitHub Issue: https://github.com/madesroches/micromegas/issues/1326

## Overview

Metrics ingested from OTel sources can report a monetary unit (e.g. `"USD"`) on the `unit` field. Today the web app has no concept of currency: an unrecognized unit like `"USD"` falls through the generic formatter and renders as a bare number with the raw unit string appended (`"1,234.56 USD"`) — no currency symbol, no fixed decimal places, no locale-aware grouping. This plan adds currency-aware formatting to the existing adaptive-unit pipeline so `USD`, `CAD`, `EUR`, and any other ISO 4217 currency code render as proper money (`"$1,234.56"`, `"CA$1,234.56"`, `"€1,234.56"`).

## Current State

The web app has one generic value-formatting pipeline that money should plug into rather than a new bespoke path:

- `analytics-web-app/src/lib/units.ts` — `UNIT_ALIASES` map + `normalizeUnit()` (units.ts:7-90) normalize raw unit strings to a canonical name. `isSizeUnit()`/`isBitUnit()` (units.ts:120-143) plus `getAdaptiveSizeUnit()`/`getAdaptiveBitUnit()` (units.ts:196-296) pick the best display unit for a representative value (e.g. bytes → GB).
- `analytics-web-app/src/lib/time-units.ts` — the same pattern for time units.
- `analytics-web-app/src/lib/format-value.ts` — the single dispatch point `formatValueWithUnit(value, rawUnit)` (format-value.ts:46-51). Internally `formatNonTime()` (format-value.ts:18-40) branches on size/bit/percent/degrees/boolean; the **fallback at format-value.ts:39** (`` return rawUnit ? `${value.toLocaleString()} ${rawUnit}` : value.toLocaleString() ``) is what currently handles `"USD"` and is exactly where a currency branch needs to be inserted, before the fallback.
- `analytics-web-app/src/lib/template-functions.ts` — exposes `format_value(value, unit)` to notebook/table templates by calling straight into `formatValueWithUnit`; currency support here is automatic once `format-value.ts` handles it.
- `analytics-web-app/src/components/XYChart.tsx` — the chart component. Relevant call sites that all funnel through the same primitives:
  - `unitScaleInfo` (XYChart.tsx:234-268) groups multi-series by raw unit string for per-unit Y axes.
  - `primaryUnit` / `adaptiveTimeUnit` / `adaptiveSizeUnit` / `adaptiveBitUnit` memos (XYChart.tsx:271-289) and `displayUnit` (XYChart.tsx:292) compute the header abbreviation.
  - Multi-axis Y-label building (XYChart.tsx:704-736): `yAxisUnit = adaptiveInfo?.abbrev ?? (scaleInfo.unitName === 'percent' ? '%' : scaleInfo.unitName)`.
  - Single-series Y-axis label (XYChart.tsx:914): same `percent → '%'` special case pattern.
  - Tooltips (XYChart.tsx:404, 474-476, 568) and stats panel (XYChart.tsx:1178-1187) all call `formatValueWithUnit(value, unit)` directly — no changes needed there once the shared formatter handles currency.
- `rust/analytics/src/lakehouse/otel/metrics_block_processor.rs` and `rust/analytics/src/metrics_table.rs` — the OTel `unit` field is parsed from the OTLP proto and stored verbatim in the `measures.unit` column (`Dictionary(Int16, Utf8)`), with no normalization or currency awareness. It flows end-to-end unmodified from ingestion to the web app's SQL query results. **No backend change is required** — the string `"USD"` (or whatever the OTel SDK sends) already reaches the frontend as-is.
- Precedent: `tasks/completed/issue_746_chart_unit_configuration_plan.md` documents the original design of this unit pipeline (bytes/bits/percent/degrees/boolean) and its stated design decision — *"No special-casing of non-units... unknown units display as `value unit`"* — money is the first case where that fallback needs an actual exception.
- Confirmed via live query against `game_metrics_per_process_per_minute` (`claude_code.cost.usage`, last 15 minutes): `unit = "USD"` — bare code, no `{}` annotation. No stripping logic is needed; the unit arrives bare.

## Design

### Currency detection

Unlike time/size/bit units, currency codes are an open set (ISO 4217 has ~180 codes) and the issue asks for USD, CAD, EUR "and other money metrics" — so this should not be a hand-maintained alias list like `UNIT_ALIASES`. Note that `Intl.NumberFormat`'s currency validation only checks that the code is 3 alphabetic characters, not actual ISO 4217 registry membership — plausible non-currency 3-letter unit abbreviations that OTel/UCUM metrics could plausibly use (`MPH`, `RPM`, `FPS`, `SEC`, `PCT`, or UCUM codes like `Cel`, `mol`, `kat`) construct an `Intl.NumberFormat` and format without throwing, so "construction succeeds" is not a valid currency test on its own. Instead, validate against the runtime's actual currency registry via `Intl.supportedValuesOf('currency')` (broadly available: Chrome 99+, Firefox 93+, Safari 15.4+, Node 18+), which enumerates real ISO 4217 codes:

```ts
// units.ts
const KNOWN_CURRENCY_CODES = new Set<string>(
  typeof Intl.supportedValuesOf === 'function' ? Intl.supportedValuesOf('currency') : []
)

export function isCurrencyUnit(unit: string): boolean {
  return KNOWN_CURRENCY_CODES.has(unit.toUpperCase())
}
```

`KNOWN_CURRENCY_CODES` is built once at module load, so no per-call caching is needed — `Set.has()` is already O(1), cheap on hot paths like tooltip re-formatting on mouse move. On a runtime that lacks `Intl.supportedValuesOf` (pre-2022 engines), the set is empty and `isCurrencyUnit` always returns `false`, so currency formatting silently degrades to the existing `value unit` fallback instead of throwing.

Note: `analytics-web-app/tsconfig.json` currently sets `"lib": ["dom", "dom.iterable", "ES2020"]`, which does not include the `ES2022.Intl` lib file that declares `Intl.supportedValuesOf`. That lib entry must be added (see Implementation Steps) or this code will not typecheck.

### Formatting

```ts
// units.ts
export function formatCurrencyValue(value: number, unit: string): string {
  return new Intl.NumberFormat(undefined, {
    style: 'currency',
    currency: unit.toUpperCase(),
  }).format(value)
}
```

- `formatCurrencyValue` is used for tooltips, the stats panel, and (see Wiring into `XYChart.tsx` below) the Y-axis ticks — full Intl-formatted amount, e.g. `"$1,234.56"`.
- No adaptive scaling (no "$1.2M" abbreviation) — matches the existing simple-unit precedent (`percent`, `degrees`) rather than the adaptive-scaling precedent (`bytes`, time). Large-number abbreviation for money can be a follow-up if requested; keeping v1 simple avoids inventing rounding/rollover rules (is $999,999 → "$1.0M" acceptable? threshold choice, etc.) that nobody has asked for yet.
- No explicit locale is passed (`undefined` locale = the browser's runtime locale), matching the existing convention already used by the `value.toLocaleString()` fallback in `format-value.ts:39`.

### Wiring into `formatNonTime`

```ts
// format-value.ts
import { isCurrencyUnit, formatCurrencyValue } from './units'
...
function formatNonTime(value: number, rawUnit: string): string {
  const unit = normalizeUnit(rawUnit)
  ...
  if (isCurrencyUnit(unit)) return formatCurrencyValue(value, unit)

  return rawUnit ? `${value.toLocaleString()} ${rawUnit}` : value.toLocaleString()
}
```

Placed just before the fallback, after the existing size/bit/percent/degrees/boolean checks (order doesn't matter for correctness since the sets are disjoint, but keeping it last-before-fallback minimizes the diff).

### Wiring into `XYChart.tsx`

The Y-axis `values` callbacks (XYChart.tsx:746-756 for multi-series, XYChart.tsx:1004-1013 for single-series) format every tick by concatenating a manually-rounded number with the unit string — e.g. `Math.round(dv) + ' ' + yAxisUnit`, `dv.toFixed(1) + ' ' + yAxisUnit`, `dv.toPrecision(2) + ' ' + yAxisUnit`. Simply swapping `yAxisUnit` for a currency symbol (as at `scaleInfo.unitName === 'percent' ? '%' : ...`) would still leave every tick using this ad-hoc rounding and symbol-suffix concatenation, rendering e.g. `"1,235 $"` instead of `"$1,235"` — inconsistent with the tooltips/stats panel, which use `formatCurrencyValue()`. So currency must bypass the manual-rounding + suffix path entirely, not just swap the unit string:

- Multi-series (XYChart.tsx:734-756): alongside the existing `yAxisUnit` computation, compute `const isCurrencyScale = isCurrencyUnit(normalizeUnit(scaleInfo.unitName))`. In the `values` callback, when `isCurrencyScale` is true, return `formatCurrencyValue(dv, scaleInfo.unitName)` for every tick (including the `v === 0` case) instead of the `Math.round`/`toFixed`/`toPrecision` + `yAxisUnit` chain.
- Single-series (XYChart.tsx:914 and :1004-1013): same pattern — `const isCurrencyScale = isCurrencyUnit(normalizeUnit(primaryUnit))`, branching inside the `values` callback at :1004-1013.

No other XYChart changes needed: tooltips (404, 474-476, 568) and the stats panel (1178-1187) already call `formatValueWithUnit`, which now handles currency via `format-value.ts`.

## Implementation Steps

1. **`analytics-web-app/src/lib/units.ts`**: add `isCurrencyUnit()`, `formatCurrencyValue()` (with the module-level `KNOWN_CURRENCY_CODES` set) as described above. Also update **`analytics-web-app/tsconfig.json`**'s `lib` array to add `"ES2022.Intl"` (required for `Intl.supportedValuesOf` to typecheck under the project's current `lib: ["dom", "dom.iterable", "ES2020"]`).
2. **`analytics-web-app/src/lib/format-value.ts`**: import the new helpers, add the currency branch in `formatNonTime()` before the fallback.
3. **`analytics-web-app/src/components/XYChart.tsx`**: import `isCurrencyUnit`, `formatCurrencyValue`; update the two Y-axis `values` tick-formatter callbacks (multi-series ~746-756, single-series ~1004-1013) to branch on `isCurrencyUnit(normalizeUnit(...))` and format currency ticks via `formatCurrencyValue()` directly, bypassing the manual `toFixed`/`toPrecision`/`Math.round` + unit-suffix chain used for other units.
4. **Tests**:
   - `analytics-web-app/src/lib/__tests__/units.test.ts`: `isCurrencyUnit` (true for `USD`/`CAD`/`EUR`/lowercase `usd`, false for `count`/`percent`/arbitrary strings, and false for plausible non-currency 3-letter unit codes that `Intl.NumberFormat` construction alone would wrongly accept — `MPH`, `RPM`, `Cel`), `formatCurrencyValue` for `USD`/`CAD`/`EUR` — assert against a dynamically-constructed `Intl.NumberFormat(undefined, { style: 'currency', currency: code })` (mirroring the existing `toLocaleString()`-based pattern in `format-value.test.ts:112,116`) rather than a pinned literal string, so the test doesn't break under a non-en-US default locale/ICU build.
   - `analytics-web-app/src/lib/__tests__/format-value.test.ts`: `formatValueWithUnit(1234.5, 'USD')` renders as currency, unknown non-currency unit still falls through to the old `value unit` behavior.
5. **Documentation**: update `mkdocs/docs/query-guide/schema-reference.md`'s `unit` column description to mention that ISO 4217 currency codes (e.g. `USD`, `CAD`, `EUR`) are recognized and rendered as money. Also update `mkdocs/docs/web-app/notebooks/variables.md:144`, which independently enumerates the `format_value()` unit vocabulary ("bytes, KB, MB, seconds, ms, µs, bits/s, percent, degrees, boolean, …") — add currency codes to that list so it doesn't go stale.

## Files to Modify

- `analytics-web-app/src/lib/units.ts`
- `analytics-web-app/tsconfig.json`
- `analytics-web-app/src/lib/format-value.ts`
- `analytics-web-app/src/components/XYChart.tsx`
- `analytics-web-app/src/lib/__tests__/units.test.ts`
- `analytics-web-app/src/lib/__tests__/format-value.test.ts`
- `mkdocs/docs/query-guide/schema-reference.md`
- `mkdocs/docs/web-app/notebooks/variables.md`

No Rust changes — the `unit` string already passes through `metrics_block_processor.rs` / `metrics_table.rs` unmodified.

## Trade-offs

- **`Intl.supportedValuesOf('currency')`-based detection vs. a hardcoded currency list**: chosen to support "other money metrics" generically per the issue, without maintaining an ISO 4217 table that will drift out of date, while still validating against the runtime's actual currency registry (unlike the naive "does `Intl.NumberFormat` construction succeed" heuristic, which also accepts non-currency 3-letter codes like `MPH`/`Cel`). Cost: relies on the runtime supporting `Intl.supportedValuesOf` (Node 18+, modern browsers) — see the fallback behavior noted in Currency detection above.
- **No adaptive large-number abbreviation** (unlike bytes/time): keeps v1 scope tight and avoids inventing rounding semantics nobody asked for. Can be added later as `getAdaptiveCurrencyUnit()` following the exact shape of `getAdaptiveSizeUnit()` if large dollar figures become common.
- **No explicit locale**: uses the viewer's browser locale (consistent with existing `toLocaleString()` fallback), so the same `"CAD"` value renders as `CA$1,234.56` in an `en-*` locale and `1 234,56 $CA` in a `fr-*` locale. This is desirable (locale-correct display) but means two viewers see different formatting for the identical raw value — acceptable since it matches how `toLocaleString()` already behaves for every other unhandled unit today.

## Documentation

- `mkdocs/docs/query-guide/schema-reference.md` — note that the `measures.unit` column may carry ISO 4217 currency codes and that the web app renders them as currency.
- `mkdocs/docs/web-app/notebooks/variables.md` — update the `format_value()` unit-vocabulary sentence (line 144) and, optionally, its examples table to mention currency codes alongside bytes/seconds/percent/etc.

## Testing Strategy

- Unit tests on the new `units.ts` helpers and the `format-value.ts` dispatch, covering `USD`, `CAD`, `EUR`, lowercase input, and a non-currency unit to confirm the fallback path is untouched.
- Manual check in the running web app: create/query a metric with `unit = "USD"` (or `"CAD"`/`"EUR"`) and confirm the chart tooltip, stats panel, and Y-axis label all render currency-formatted values instead of `"1,234.56 USD"`.
