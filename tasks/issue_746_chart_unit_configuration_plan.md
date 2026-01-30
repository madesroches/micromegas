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

### Step 1: Create `units.ts` with `normalizeUnit()` function

Create `analytics-web-app/src/lib/units.ts`:

```typescript
const UNIT_ALIASES: Record<string, string> = {
  // Time (include canonical names for case-insensitive matching)
  'ns': 'nanoseconds',
  'nanoseconds': 'nanoseconds',
  'Nanoseconds': 'nanoseconds',
  'µs': 'microseconds',
  'us': 'microseconds',
  'microseconds': 'microseconds',
  'Microseconds': 'microseconds',
  'ms': 'milliseconds',
  'milliseconds': 'milliseconds',
  'Milliseconds': 'milliseconds',
  's': 'seconds',
  'seconds': 'seconds',
  'Seconds': 'seconds',
  'min': 'minutes',
  'minutes': 'minutes',
  'Minutes': 'minutes',
  'h': 'hours',
  'hours': 'hours',
  'Hours': 'hours',
  'd': 'days',
  'days': 'days',
  'Days': 'days',
  // Size
  'bytes': 'bytes',
  'Bytes': 'bytes',
  'B': 'bytes',
  'kilobytes': 'kilobytes',
  'Kilobytes': 'kilobytes',
  'KB': 'kilobytes',
  'kb': 'kilobytes',
  'megabytes': 'megabytes',
  'Megabytes': 'megabytes',
  'MB': 'megabytes',
  'gigabytes': 'gigabytes',
  'Gigabytes': 'gigabytes',
  'GB': 'gigabytes',
  // Rate
  'BytesPerSecond': 'bytes/s',
  'BytesPerSeconds': 'bytes/s',
  'B/s': 'bytes/s',
  'bytes/s': 'bytes/s',
  // Other
  'requests': 'count',
  'count': 'count',
  '%': 'percent',
  'percent': 'percent',
  'deg': 'degrees',
  'degrees': 'degrees',
  'boolean': 'boolean',
  'none': 'none',
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
  if (unit === 'megabytes') {
    if (value >= 1e3) return (value / 1e3).toFixed(1) + ' GB'
    return value.toFixed(1) + ' MB'
  }
  if (unit === 'gigabytes') {
    return value.toFixed(1) + ' GB'
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
  if (unit === 'boolean') return value !== 0 ? 'true' : 'false'
  if (unit === 'none' || unit === '') return value.toFixed(2)

  // Custom fallback
  return value.toFixed(2) + ' ' + rawUnit  // Use original for display
}
```

### Step 3: Update `isTimeUnit()` check

Create a `TIME_UNITS` set and check against normalized units:

```typescript
const TIME_UNITS = new Set([
  'nanoseconds',
  'microseconds',
  'milliseconds',
  'seconds',
  'minutes',
  'hours',
  'days',
])

function isTimeUnit(unit: string): boolean {
  return TIME_UNITS.has(normalizeUnit(unit))
}
```

### Step 4: Update adaptive time unit calculation

Ensure `getAdaptiveTimeUnit()` works with normalized units.

## Files Changed

| File | Change |
|------|--------|
| `analytics-web-app/src/lib/units.ts` | New file: `UNIT_ALIASES`, `normalizeUnit()`, `TIME_UNITS`, `isTimeUnit()` |
| `analytics-web-app/src/components/XYChart.tsx` | Import from `units.ts`, update `formatValue()` to use normalized units |

## Test Cases

```typescript
// Time aliases
formatValue(100, 'ns')           // adaptive time
formatValue(100, 's')            // adaptive time
formatValue(100, 'Milliseconds') // adaptive time
formatValue(100, 'Seconds')      // adaptive time

// Size aliases
formatValue(100, 'Bytes')        // "100 B"
formatValue(100, 'B')            // "100 B"
formatValue(1500, 'Kilobytes')   // "1.5 MB"
formatValue(1500, 'KB')          // "1.5 MB"
formatValue(1500, 'megabytes')   // "1.5 GB"
formatValue(1500, 'MB')          // "1.5 GB"
formatValue(2.5, 'gigabytes')    // "2.5 GB"
formatValue(2.5, 'GB')           // "2.5 GB"

// Rate units
formatValue(1500000, 'BytesPerSeconds') // "1.5 MB/s"
formatValue(1500000, 'B/s')             // "1.5 MB/s"

// Other units
formatValue(90, 'deg')           // "90°"
formatValue(90, 'degrees')       // "90°"
formatValue(42, 'none')          // "42.00"
formatValue(1234, 'requests')    // "1,234"
formatValue(1234, 'count')       // "1,234"
formatValue(75.5, 'percent')     // "75.5%"
formatValue(75.5, '%')           // "75.5%"

// Boolean (explicit zero check)
formatValue(1, 'boolean')        // "true"
formatValue(0, 'boolean')        // "false"
formatValue(0.5, 'boolean')      // "true" (non-zero)
```

## Estimate

~80-120 lines of code: ~60 lines in new `units.ts`, ~20-60 lines updated in `XYChart.tsx`.
