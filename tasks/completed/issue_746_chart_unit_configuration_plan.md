# Issue #746: Chart Unit Configuration Plan

GitHub Issue: https://github.com/madesroches/micromegas/issues/746

**Status: COMPLETED**

## Summary

Added alias support and formatters so units in various formats work correctly in charts.

## What Was Implemented

### New file: `analytics-web-app/src/lib/units.ts`
- `UNIT_ALIASES` map for normalizing unit strings to canonical forms
- `normalizeUnit()` function
- `TIME_UNIT_NAMES` set for time unit detection

### Updated: `analytics-web-app/src/components/XYChart.tsx`
- Uses `normalizeUnit()` for consistent unit handling
- Added formatters for: kilobytes, megabytes, gigabytes, bytes/s, degrees, boolean
- Simplified default case: unknown units display as `value unit`

### Updated: `analytics-web-app/src/lib/time-units.ts`
- `isTimeUnit()` now accepts aliases (e.g., `ns`, `ms`, `Seconds`)
- `getAdaptiveTimeUnit()` and `formatTimeValue()` accept aliases

### New tests
- `analytics-web-app/src/lib/__tests__/units.test.ts`
- `analytics-web-app/src/lib/__tests__/time-units.test.ts`

## Supported Unit Aliases

| User Input | Canonical | Format |
|------------|-----------|--------|
| `ns`, `Nanoseconds` | `nanoseconds` | Adaptive time |
| `µs`, `us`, `Microseconds` | `microseconds` | Adaptive time |
| `ms`, `Milliseconds` | `milliseconds` | Adaptive time |
| `s`, `Seconds` | `seconds` | Adaptive time |
| `min`, `Minutes` | `minutes` | Adaptive time |
| `h`, `Hours` | `hours` | Adaptive time |
| `d`, `Days` | `days` | Adaptive time |
| `B`, `Bytes` | `bytes` | Adaptive size (B → KB → MB → GB) |
| `KB`, `kb`, `Kilobytes` | `kilobytes` | Adaptive size (from KB) |
| `MB`, `Megabytes` | `megabytes` | Adaptive size (from MB) |
| `GB`, `Gigabytes` | `gigabytes` | Shows as GB |
| `B/s`, `BytesPerSecond`, `BytesPerSeconds` | `bytes/s` | Adaptive rate |
| `%` | `percent` | `value%` |
| `deg` | `degrees` | `value°` |
| `boolean` | `boolean` | `true`/`false` |
| (unknown) | (passthrough) | `value unit` |

## Design Decisions

- **No special-casing of non-units**: `count`, `requests`, `none` are not normalized - they display as-is like any unknown unit
- **Default uses `toLocaleString()`**: Clean number formatting with thousand separators
- **Original unit preserved in display**: Unknown units show the original string, not normalized form
