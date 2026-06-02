# Time Format in Web App Plan — DONE

## Overview

Durations displayed in the flamegraph tooltip cap out at seconds (`2500ms → 2.50s`). Spans lasting minutes, hours, or days are unreadable. The fix is to route `formatDuration` through the existing `formatTimeValue` utility, which already handles the full ns→days range.

## Current State

### `formatDuration` — `flame-model.ts:294-299`

```ts
export function formatDuration(ms: number): string {
  if (ms < 0.001) return `${(ms * 1_000_000).toFixed(0)}ns`
  if (ms < 1)     return `${(ms * 1000).toFixed(0)}us`
  if (ms < 1000)  return `${ms.toFixed(1)}ms`
  return `${(ms / 1000).toFixed(2)}s`   // ← stops here, no minutes/hours/days
}
```

Called from `FlameGraphCell.tsx:277`:
```ts
info += `<br>Duration: ${formatDuration(end - begin)}`
```

### `formatTimeValue` — `time-units.ts:163-170`

Already handles the full range (ns → days) via `getAdaptiveTimeUnit`. Accepts a value and its unit string (`'ms'`, `'s'`, etc.) and an `abbreviated` flag:

```ts
formatTimeValue(120_000, 'ms', true)  // → "2.00 min"
formatTimeValue(7_200_000, 'ms', true) // → "2.00 h"
```

The function is already tested in `lib/__tests__/time-units.test.ts`.

### Minor style differences

| Input | `formatDuration` (current) | `formatTimeValue(v, 'ms', true)` |
|-------|---------------------------|-----------------------------------|
| 0.0005 ms | `500ns` | `500 ns` |
| 0.5 ms | `500us` | `500 µs` |
| 12.34 ms | `12.3ms` | `12.3 ms` |
| 2500 ms | `2.50s` | `2.50 s` |
| 120_000 ms | `120.s` (broken) | `2.00 min` |

The new output adds a space and uses `µs` instead of `us` — both improvements.

## Design

Replace the body of `formatDuration` with a delegation to `formatTimeValue`:

```ts
import { formatTimeValue } from '@/lib/time-units'

export function formatDuration(ms: number): string {
  return formatTimeValue(ms, 'milliseconds', true)
}
```

Keeping `formatDuration` as a named wrapper preserves the call site in `FlameGraphCell.tsx` and all import paths unchanged.

## Implementation Steps

1. **`flame-model.ts`**: Replace the body of `formatDuration` (lines 294–299) with `return formatTimeValue(ms, 'milliseconds', true)`. Add the import from `@/lib/time-units`. ✓
2. **`flame-model.test.ts`**: Update the `formatDuration` test cases to match new output (space-separated unit, `µs` instead of `us`). Add a case for `> 60_000 ms` (minutes) and `> 3_600_000 ms` (hours). ✓

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/cells/flame-model.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/flame-model.test.ts`

## Trade-offs

- **Reuse vs. keep inline**: delegating to `formatTimeValue` avoids duplicating the breakpoint logic. Downside: a small coupling between the flamegraph layer and `time-units`. This coupling is intentional — both care about the same domain.
- **Keep `formatDuration` vs. inline call**: keeping the wrapper means zero changes to call sites and keeps the intent explicit.

## Testing Strategy

- Run `yarn test` in `analytics-web-app/` — updated unit tests cover ns, µs, ms, s, min, h.
- Manually hover a long-running span (> 60 s) in the flamegraph to confirm the tooltip shows minutes.
