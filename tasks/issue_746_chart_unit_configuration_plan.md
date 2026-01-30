# Issue #746: Chart Unit Configuration Plan

GitHub Issue: https://github.com/madesroches/micromegas/issues/746

## Summary

Add alias support and a few missing formatters so all units in use work correctly.

## Current State (Already Done)

- Unit input field in chart cell editor
- Macro substitution (`$variable.unit`)
- Y-axis labels display units
- Adaptive time formatting (ns → µs → ms → s → min → h → d)
- Adaptive byte formatting (B → KB → MB → GB)
- Percent and count formatting
- Custom unit fallback

## The Gap

Current code uses **exact string matching**. Units like `Bytes`, `ns`, `s`, `Milliseconds`, `BytesPerSeconds` don't work.

## Units to Support

| User Input | Canonical | Format |
|------------|-----------|--------|
| `ns`, `nanoseconds` | `nanoseconds` | Adaptive time |
| `ms`, `Milliseconds`, `milliseconds` | `milliseconds` | Adaptive time |
| `s`, `Seconds`, `seconds` | `seconds` | Adaptive time |
| `bytes`, `Bytes`, `B` | `bytes` | Adaptive size |
| `Kilobytes`, `KB`, `kb` | `kilobytes` | Adaptive size (from KB) |
| `BytesPerSeconds`, `B/s` | `bytes/s` | Adaptive size + `/s` |
| `percent`, `%` | `percent` | `value%` |
| `count`, `requests` | `count` | Integer with commas |
| `deg`, `degrees` | `degrees` | `value°` |
| `m`, `meters` | `meters` | `value m` |
| `none`, `` | `none` | Just the number |
| `boolean` | `boolean` | `true`/`false` |

## Implementation

### Step 1: Add `normalizeUnit()` function

In `XYChart.tsx` (or a small `units.ts` helper):

```typescript
const UNIT_ALIASES: Record<string, string> = {
  // Time
  'ns': 'nanoseconds',
  'µs': 'microseconds',
  'us': 'microseconds',
  'ms': 'milliseconds',
  'Milliseconds': 'milliseconds',
  's': 'seconds',
  'Seconds': 'seconds',
  'min': 'minutes',
  'h': 'hours',
  'd': 'days',
  // Size
  'Bytes': 'bytes',
  'B': 'bytes',
  'Kilobytes': 'kilobytes',
  'KB': 'kilobytes',
  'kb': 'kilobytes',
  'MB': 'megabytes',
  'GB': 'gigabytes',
  // Rate
  'BytesPerSeconds': 'bytes/s',
  'B/s': 'bytes/s',
  // Other
  'requests': 'count',
  '%': 'percent',
  'deg': 'degrees',
  'degrees': 'degrees',
}

function normalizeUnit(unit: string): string {
  return UNIT_ALIASES[unit] ?? unit
}
```

### Step 2: Update `formatValue()`

```typescript
function formatValue(value: number, rawUnit: string, ...): string {
  const unit = normalizeUnit(rawUnit)

  // Time units (existing, works with normalized unit)
  if (adaptiveTimeUnit && isTimeUnit(unit)) {
    return formatAdaptiveTime(value, adaptiveTimeUnit, abbreviated)
  }

  // Size units
  if (unit === 'bytes') {
    // existing code
  }
  if (unit === 'kilobytes') {
    if (value >= 1e6) return (value / 1e6).toFixed(1) + ' GB'
    if (value >= 1e3) return (value / 1e3).toFixed(1) + ' MB'
    return value.toFixed(1) + ' KB'
  }

  // Rate units
  if (unit === 'bytes/s') {
    if (value >= 1e9) return (value / 1e9).toFixed(1) + ' GB/s'
    if (value >= 1e6) return (value / 1e6).toFixed(1) + ' MB/s'
    if (value >= 1e3) return (value / 1e3).toFixed(1) + ' KB/s'
    return value.toFixed(0) + ' B/s'
  }

  // Other units
  if (unit === 'percent') return value.toFixed(1) + '%'
  if (unit === 'count') return Math.round(value).toLocaleString()
  if (unit === 'degrees') return value.toFixed(1) + '°'
  if (unit === 'boolean') return value ? 'true' : 'false'
  if (unit === 'none' || unit === '') return value.toFixed(2)

  // Custom fallback
  return value.toFixed(2) + ' ' + rawUnit  // Use original for display
}
```

### Step 3: Update `isTimeUnit()` check

Either:
- Normalize before calling `isTimeUnit()`, or
- Expand `isTimeUnit()` to accept aliases

### Step 4: Update adaptive time unit calculation

Ensure `getAdaptiveTimeUnit()` works with normalized units.

## Files Changed

| File | Change |
|------|--------|
| `analytics-web-app/src/components/XYChart.tsx` | Add `normalizeUnit()`, expand `formatValue()` |
| `analytics-web-app/src/lib/time-units.ts` | Optional: add alias support to `isTimeUnit()` |

## Test Cases

```typescript
// Aliases
formatValue(100, 'Bytes')      // "100 B"
formatValue(100, 'ns')         // adaptive time
formatValue(100, 's')          // adaptive time
formatValue(100, 'Milliseconds') // adaptive time

// New units
formatValue(1500, 'Kilobytes')      // "1.5 MB"
formatValue(1500000, 'BytesPerSeconds') // "1.5 MB/s"
formatValue(90, 'deg')              // "90°"
formatValue(1, 'boolean')           // "true"
formatValue(42, 'none')             // "42"
formatValue(1234, 'requests')       // "1,234"
```

## Estimate

~50-100 lines of code changes, mostly in `XYChart.tsx`.
