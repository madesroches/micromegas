# Time Range Zoom Buttons Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/773

## Overview

Add zoom in/out buttons to the time range control in the analytics web app header. Zoom out doubles the visible time range (centered on current view), zoom in halves it (centered on current view). This complements the existing drag-to-zoom on charts with a quick toolbar action.

## Current State

The time range controls live in the `Header` component (`src/components/layout/Header.tsx:62-77`). The layout is a button group:

```
[TimeRangePicker (rounded-left)] [Refresh (rounded-right)]
```

`TimeRangePicker` renders a button with `rounded-l-md` (`index.tsx:191`), and the Refresh button has `rounded-r-md` (`Header.tsx:72`). They share `bg-theme-border` styling and sit flush via `border-l`.

Time range state is string-based (`from`/`to`), supporting both relative (`"now-1h"`) and absolute (ISO date) formats. The `parseTimeRange` function in `src/lib/time-range.ts:123` converts strings to `Date` objects. The `onTimeRangeChange(from, to)` callback flows from page → PageLayout → Header → TimeRangePicker.

## Grafana Reference

Grafana's time range controls use a similar pattern:

```
[ << ] [ clock  Last 6 hours ] [ >> ] [ zoom-out ]
```

Key observations from Grafana's implementation (`timePicker.ts`):
- **Zoom out only** in the header toolbar — zoom in is done exclusively via drag-to-select on chart panels
- **Center-based zoom** with factor 2: `center = to - timespan/2`, then `from = center - newTimespan/2`, `to = center + newTimespan/2`
- **Converts to absolute time** on zoom (disables relative "last N" ranges)
- **Edge case**: if timespan is 0, forces zoom out to 30 seconds
- **Shift buttons** (`<<` / `>>`) move the window by half the current span
- **Keyboard shortcuts**: `t -` (zoom out), `t +` (zoom in)

Our issue asks for both zoom in and zoom out buttons, which is a departure from Grafana (header zoom-out only). This is reasonable since our app already has drag-to-zoom on charts for precise selection, and the header buttons provide coarser quick navigation in both directions.

## Design

### Button Placement

Insert zoom out (−) and zoom in (+) buttons between the TimeRangePicker and the Refresh button, forming a continuous button group:

```
[TimeRangePicker] [ZoomOut] [ZoomIn] [Refresh]
 rounded-l-md      flat       flat     rounded-r-md
```

All buttons share the same `bg-theme-border` / `hover:bg-theme-border-hover` styling with `border-l` separators, matching the existing Refresh button pattern.

### Zoom Logic

Add a `zoomTimeRange` function to `src/lib/time-range.ts`, following the same algorithm as Grafana:

1. Parse current `from`/`to` to absolute `Date` objects via `parseTimeRange`
2. Compute the current duration: `duration = to - from`
3. Handle zero-duration edge case: if duration is 0, default to 30 seconds
4. Compute the center: `center = from + duration / 2`
5. For zoom out: new duration = `duration * 2`, for zoom in: new duration = `duration / 2`
6. Compute new from/to: `center - newDuration/2` and `center + newDuration/2`
7. Clamp `to` so it doesn't exceed `now` (no future time ranges)
8. Return as ISO strings (zoom always produces absolute time ranges)

The function converts relative ranges to absolute on zoom, which is the expected behavior — once you zoom, you're working with a specific time window, not a sliding "last N hours". This matches Grafana's behavior.

### Minimum/Maximum Duration

- Minimum zoom-in duration: 10 seconds (prevent zooming into meaninglessly small ranges)
- Maximum zoom-out duration: 365 days (practical upper bound)

### Icons

Use `ZoomIn` and `ZoomOut` from `lucide-react` (already a project dependency).

## Implementation Steps

### Step 1: Add zoom utility function

**File**: `src/lib/time-range.ts`

Add `zoomTimeRange(from: string, to: string, direction: 'in' | 'out'): { from: string; to: string }` that:
- Parses the current range to dates
- Computes centered zoom (2x out, 0.5x in)
- Clamps to [10s, 365d] duration and caps `to` at `now`
- Returns ISO string pair

### Step 2: Add zoom buttons to Header

**File**: `src/components/layout/Header.tsx`

- Import `ZoomIn`, `ZoomOut` from `lucide-react`
- Import `zoomTimeRange` from `@/lib/time-range`
- Add two buttons between the TimeRangePicker and Refresh button
- Each button calls `zoomTimeRange` then `timeRangeControl.onTimeRangeChange`
- Style to match the existing button group (same height, bg, border-l separator, no rounding on these middle buttons)
- Remove `rounded-l-md` from TimeRangePicker button and `rounded-r-md` from Refresh button — no, actually TimeRangePicker's rounding is internal to its component. Instead:
  - TimeRangePicker keeps `rounded-l-md` (it's the leftmost)
  - Zoom buttons get no rounding, just `border-l`
  - Refresh keeps `rounded-r-md` (it's the rightmost)

### Step 3: Add tests

**File**: `src/lib/__tests__/time-range.test.ts` (new or extend existing)

Test `zoomTimeRange`:
- Zoom out doubles duration, centered
- Zoom in halves duration, centered
- Zoom from relative range produces absolute range
- `to` is clamped to not exceed current time
- Minimum duration enforced on zoom in
- Maximum duration enforced on zoom out

## Files to Modify

- `src/lib/time-range.ts` — add `zoomTimeRange` function
- `src/components/layout/Header.tsx` — add zoom buttons to the button group

## Trade-offs

**Absolute vs relative output**: Zoom always produces absolute ISO strings. An alternative would be to try to find the nearest relative preset after zoom (e.g., zooming out from "Last 1 hour" → "Last 2 hours"), but this adds complexity for little benefit — the user can always pick a preset from the dropdown if they want a relative range.

**Center-based zoom**: The issue specifies centering on current view. An alternative is to anchor the `to` edge (useful when watching live data), but centering matches the stated requirement and is the more intuitive behavior for exploring historical data.

## Testing Strategy

1. Unit test `zoomTimeRange` with various inputs (relative, absolute, edge cases)
2. Manual testing: verify buttons appear correctly in the header on all pages that show time controls
3. Verify zoom in/out triggers data reload (existing `useEffect` on `apiTimeRange` changes handles this)
4. Verify URL updates correctly after zoom (existing `updateConfig` flow handles this)
