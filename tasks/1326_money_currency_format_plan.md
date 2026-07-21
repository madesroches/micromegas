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

## Design

### Currency detection

Unlike time/size/bit units, currency codes are an open set (ISO 4217 has ~180 codes) and the issue asks for USD, CAD, EUR "and other money metrics" — so this should not be a hand-maintained alias list like `UNIT_ALIASES`. Instead, detect a currency unit by asking the platform: attempt to construct an `Intl.NumberFormat` with `{ style: 'currency', currency: code }` and treat success as "this is a currency code." This is generic (supports any current ISO 4217 code without us maintaining a list) and self-validating (rejects garbage like `"count"` or `"requests"`).

```ts
// units.ts
const currencyValidityCache = new Map<string, boolean>()

export function isCurrencyUnit(unit: string): boolean {
  const code = unit.toUpperCase()
  let valid = currencyValidityCache.get(code)
  if (valid === undefined) {
    try {
      new Intl.NumberFormat(undefined, { style: 'currency', currency: code })
      valid = /^[A-Z]{3}$/.test(code)
    } catch {
      valid = false
    }
    currencyValidityCache.set(code, valid)
  }
  return valid
}
```

The explicit `/^[A-Z]{3}$/` check guards against `Intl.NumberFormat` accepting non-ISO-shaped strings in some engines; combined with the try/catch it keeps false positives out (e.g. a metric literally named `"pct"` won't accidentally format as currency). The cache keeps this cheap on hot paths (tooltips re-format on every mouse move).

### Formatting

```ts
// units.ts
export function formatCurrencyValue(value: number, unit: string): string {
  return new Intl.NumberFormat(undefined, {
    style: 'currency',
    currency: unit.toUpperCase(),
  }).format(value)
}

export function getCurrencySymbol(unit: string): string {
  const parts = new Intl.NumberFormat(undefined, {
    style: 'currency',
    currency: unit.toUpperCase(),
    minimumFractionDigits: 0,
    maximumFractionDigits: 0,
  }).formatToParts(0)
  return parts.find((p) => p.type === 'currency')?.value ?? unit.toUpperCase()
}
```

- `formatCurrencyValue` is used for tooltips and the stats panel (full formatted amount, e.g. `"$1,234.56"`).
- `getCurrencySymbol` is used for the Y-axis label, mirroring how `percent` collapses to `'%'` — money collapses to its symbol (`$`, `CA$`, `€`) rather than repeating the full formatted number on every axis tick.
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

Add a currency check alongside the existing `percent` special case at the two Y-axis label sites:

- XYChart.tsx:735 — `const yAxisUnit = adaptiveInfo?.abbrev ?? (scaleInfo.unitName === 'percent' ? '%' : isCurrencyUnit(scaleInfo.unitName) ? getCurrencySymbol(scaleInfo.unitName) : scaleInfo.unitName)`
- XYChart.tsx:914 — same pattern using `primaryUnit`.

No other XYChart changes needed: tooltips (404, 474-476, 568) and the stats panel (1178-1187) already call `formatValueWithUnit`, which now handles currency via `format-value.ts`.

## Implementation Steps

1. **`analytics-web-app/src/lib/units.ts`**: add `isCurrencyUnit()`, `formatCurrencyValue()`, `getCurrencySymbol()` (with the validity cache) as described above.
2. **`analytics-web-app/src/lib/format-value.ts`**: import the new helpers, add the currency branch in `formatNonTime()` before the fallback.
3. **`analytics-web-app/src/components/XYChart.tsx`**: import `isCurrencyUnit`, `getCurrencySymbol`; update the two Y-axis label expressions (lines ~735 and ~914) to fall back to the currency symbol instead of the raw unit string.
4. **Tests**:
   - `analytics-web-app/src/lib/__tests__/units.test.ts`: `isCurrencyUnit` (true for `USD`/`CAD`/`EUR`/lowercase `usd`, false for `count`/`percent`/arbitrary strings), `formatCurrencyValue` (assert exact output for `USD`/`CAD`/`EUR` — pin to the current Node/ICU output, e.g. `formatCurrencyValue(1234.5, 'USD') === '$1,234.50'`), `getCurrencySymbol` (`USD` → `$`, `CAD` → `CA$`, `EUR` → `€`).
   - `analytics-web-app/src/lib/__tests__/format-value.test.ts`: `formatValueWithUnit(1234.5, 'USD')` renders as currency, unknown non-currency unit still falls through to the old `value unit` behavior.
5. **Documentation**: update `mkdocs/docs/query-guide/schema-reference.md`'s `unit` column description to mention that ISO 4217 currency codes (e.g. `USD`, `CAD`, `EUR`) are recognized and rendered as money.

## Files to Modify

- `analytics-web-app/src/lib/units.ts`
- `analytics-web-app/src/lib/format-value.ts`
- `analytics-web-app/src/components/XYChart.tsx`
- `analytics-web-app/src/lib/__tests__/units.test.ts`
- `analytics-web-app/src/lib/__tests__/format-value.test.ts`
- `mkdocs/docs/query-guide/schema-reference.md`

No Rust changes — the `unit` string already passes through `metrics_block_processor.rs` / `metrics_table.rs` unmodified.

## Trade-offs

- **`Intl.NumberFormat`-based detection vs. a hardcoded currency list**: chosen to support "other money metrics" generically per the issue, without maintaining an ISO 4217 table that will drift out of date. Cost: relies on the runtime's ICU data having the currency (all currencies clients realistically emit — USD/CAD/EUR and other major codes — are covered by any modern browser/Node ICU build).
- **No adaptive large-number abbreviation** (unlike bytes/time): keeps v1 scope tight and avoids inventing rounding semantics nobody asked for. Can be added later as `getAdaptiveCurrencyUnit()` following the exact shape of `getAdaptiveSizeUnit()` if large dollar figures become common.
- **No explicit locale**: uses the viewer's browser locale (consistent with existing `toLocaleString()` fallback), so the same `"CAD"` value renders as `CA$1,234.56` in an `en-*` locale and `1 234,56 $CA` in a `fr-*` locale. This is desirable (locale-correct display) but means two viewers see different formatting for the identical raw value — acceptable since it matches how `toLocaleString()` already behaves for every other unhandled unit today.

## Documentation

- `mkdocs/docs/query-guide/schema-reference.md` — note that the `measures.unit` column may carry ISO 4217 currency codes and that the web app renders them as currency.

## Testing Strategy

- Unit tests on the new `units.ts` helpers and the `format-value.ts` dispatch, covering `USD`, `CAD`, `EUR`, lowercase input, and a non-currency unit to confirm the fallback path is untouched.
- Manual check in the running web app: create/query a metric with `unit = "USD"` (or `"CAD"`/`"EUR"`) and confirm the chart tooltip, stats panel, and Y-axis label all render currency-formatted values instead of `"1,234.56 USD"`.

## Open Questions

- ~~Does the OTel SDK actually send the bare code `"USD"` on the `unit` field, or a UCUM-annotated form like `"{USD}"`?~~ **Confirmed via live query** against `game_metrics_per_process_per_minute` (`claude_code.cost.usage`, last 15 minutes): `unit = "USD"` — bare code, no `{}` annotation. No stripping logic needed.
- Should currency detection be case-sensitive to match ISO 4217 (`USD` only) or lenient (`usd`, `Usd`)? This plan treats it leniently (uppercases before validating), matching the leniency already shown for other units in `UNIT_ALIASES` (e.g. `Bytes`/`bytes`/`B`).
