# Time Range Zoom Buttons Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/773
**Status**: Implemented

## Overview

Add zoom in/out buttons to the time range control in the analytics web app header. Zoom out doubles the visible time range (centered on current view), zoom in halves it (centered on current view). This complements the existing drag-to-zoom on charts with a quick toolbar action.

## Design

### Button Placement

Zoom out and zoom in buttons sit between the TimeRangePicker and the Refresh button, forming a continuous button group:

```
[TimeRangePicker] [ZoomOut] [ZoomIn] [Refresh]
 rounded-l-md      flat       flat     rounded-r-md
```

All buttons share the same `bg-theme-border` / `hover:bg-theme-border-hover` styling with `border-l` separators.

### Zoom Logic

The `zoomTimeRange` function in `src/lib/time-range.ts`:

1. Parses current `from`/`to` to absolute `Date` objects via `parseTimeRange`
2. Computes the current duration: `duration = to - from`
3. Handles zero-duration edge case: if duration is 0, defaults to 30 seconds
4. Computes the center: `center = from + duration / 2`
5. For zoom out: new duration = `duration * 2`, for zoom in: new duration = `duration / 2`
6. Computes new from/to: `center - newDuration/2` and `center + newDuration/2`
7. Clamps `to` so it doesn't exceed `now` (no future time ranges)
8. Returns as ISO strings (zoom always produces absolute time ranges)

Relative ranges are converted to absolute on zoom — once you zoom, you're working with a specific time window, not a sliding "last N hours". This matches Grafana's behavior.

### Duration Limits

- Minimum zoom-in duration: 1 millisecond
- Maximum zoom-out duration: 365 days

### Icons

`ZoomIn` and `ZoomOut` from `lucide-react`.

## Files Modified

- `src/lib/time-range.ts` — added `zoomTimeRange` function
- `src/components/layout/Header.tsx` — added zoom buttons to the button group
- `src/lib/__tests__/time-range-zoom.test.ts` — unit tests for `zoomTimeRange`

## Trade-offs

**Absolute vs relative output**: Zoom always produces absolute ISO strings. An alternative would be to try to find the nearest relative preset after zoom (e.g., zooming out from "Last 1 hour" → "Last 2 hours"), but this adds complexity for little benefit — the user can always pick a preset from the dropdown if they want a relative range.

**Center-based zoom**: The zoom centers on the current view. An alternative is to anchor the `to` edge (useful when watching live data), but centering is the more intuitive behavior for exploring historical data.

## Grafana Reference

Grafana's time range controls use a similar pattern but with zoom out only in the header toolbar — zoom in is done exclusively via drag-to-select on chart panels. We provide both directions in the header since the buttons offer coarser quick navigation that complements the existing drag-to-zoom on charts.
