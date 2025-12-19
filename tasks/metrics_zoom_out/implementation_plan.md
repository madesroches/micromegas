# Metrics Zoom Out - Implementation Plan

Based on mockup: `mockup_h_picker_with_toggle.html`

## Overview

Enhance the TimeRangePicker to be process-aware on process-specific screens (ProcessMetricsPage, ProcessLogPage, PerformanceAnalysisPage). When viewing a process, the picker gains:

1. Process timeline visualization with draggable viewport
2. First/Last toggle for quick ranges
3. Boundary enforcement (can't exceed process lifetime)
4. "Full Process" quick action

### Key Behavior: Live vs Dead Processes

- **Live process** (last_update_time < 2 minutes ago): "Last X" ranges use `now` as end time
- **Dead process**: "Last X" ranges use `last_update_time` as end time
- **First X** ranges always anchor to `start_time`

---

## Implementation Steps

### 1. Create ProcessContext type and hook

**File:** `src/hooks/useProcessContext.ts`

```typescript
interface ProcessContext {
  processId: string
  startTime: Date
  endTime: Date      // last_update_time or now if live
  isLive: boolean
  durationMs: number
}
```

- Fetch process start_time and last_update_time
- Determine if live (last_update_time within 2 minutes of now)
- Provide context to TimeRangePicker

### 2. Add process context to TimeRangePicker

**File:** `src/components/layout/TimeRangePicker/index.tsx`

- Accept optional `processContext` prop
- When present, render enhanced process-aware UI
- When absent, render standard picker (unchanged behavior)

### 3. Create ProcessTimeline component

**File:** `src/components/layout/TimeRangePicker/ProcessTimeline.tsx`

- Visual timeline bar showing full process lifetime
- Highlighted viewport showing current selection
- Draggable viewport to pan
- Draggable edges to resize selection
- Click on track to jump to that position
- Labels showing start/end times and total duration

### 4. Create QuickRangesWithToggle component

**File:** `src/components/layout/TimeRangePicker/QuickRangesWithToggle.tsx`

- First/Last toggle switch (default: Last)
- Range buttons that adapt labels based on toggle state
- Disable ranges that exceed process lifetime
- "Full Process" button at bottom

**Range calculation:**
```typescript
// Last X (e.g., "Last 1 hour")
if (isLive) {
  from = "now-1h"
  to = "now"
} else {
  from = new Date(endTime.getTime() - 1 * 60 * 60 * 1000).toISOString()
  to = endTime.toISOString()
}

// First X (e.g., "First 1 hour")
from = startTime.toISOString()
to = new Date(startTime.getTime() + 1 * 60 * 60 * 1000).toISOString()
```

### 5. Add boundary enforcement to CustomRange

**File:** `src/components/layout/TimeRangePicker/CustomRange.tsx`

- Clamp entered times to process boundaries
- Show warning when input is outside bounds
- Adjust input value and show notice: "Range limited to process lifetime"

### 6. Update time-range-history for process context

**File:** `src/lib/time-range-history.ts`

- Store anchor type (first/last) with recent ranges
- Display correctly in "Recent" section

### 7. Wire up process context in page components

**Files:**
- `src/routes/ProcessMetricsPage.tsx`
- `src/routes/ProcessLogPage.tsx`
- `src/routes/PerformanceAnalysisPage.tsx`

- Fetch process info (already done in most pages)
- Pass processContext to PageLayout or Header
- Header passes to TimeRangePicker

### 8. Update Header to accept process context

**File:** `src/components/layout/Header.tsx`

- Accept optional `processContext` prop
- Show process indicator badge on time picker button when active
- Pass context to TimeRangePicker

---

## Component Tree

```
Header
└── TimeRangePicker
    ├── TimePickerButton (with optional process badge)
    └── TimePickerPopover
        ├── ProcessTimeline (if processContext)
        │   ├── TimelineTrack
        │   └── TimelineViewport (draggable)
        ├── QuickRangesWithToggle (if processContext)
        │   ├── AnchorToggle (First/Last)
        │   ├── RangeButtons
        │   └── FullProcessButton
        ├── QuickRanges (if no processContext, existing)
        ├── CustomRange (enhanced with boundary enforcement)
        └── RecentRanges
```

---

## File Changes Summary

| File | Change |
|------|--------|
| `src/hooks/useProcessContext.ts` | New - process context hook |
| `src/components/layout/TimeRangePicker/index.tsx` | Modify - accept processContext prop |
| `src/components/layout/TimeRangePicker/ProcessTimeline.tsx` | New - timeline visualization |
| `src/components/layout/TimeRangePicker/QuickRangesWithToggle.tsx` | New - toggle + ranges |
| `src/components/layout/TimeRangePicker/CustomRange.tsx` | Modify - boundary enforcement |
| `src/components/layout/Header.tsx` | Modify - pass processContext |
| `src/routes/ProcessMetricsPage.tsx` | Modify - provide processContext |
| `src/routes/ProcessLogPage.tsx` | Modify - provide processContext |
| `src/routes/PerformanceAnalysisPage.tsx` | Modify - provide processContext |
| `src/lib/time-range-history.ts` | Modify - store anchor type |

---

## Testing Checklist

- [ ] Live process: "Last 1h" uses `now-1h` to `now`
- [ ] Dead process: "Last 1h" uses `end-1h` to `end`
- [ ] "First 1h" always uses `start` to `start+1h`
- [ ] Toggle switches all labels and recalculates ranges
- [ ] Ranges exceeding process lifetime are disabled
- [ ] "Full Process" selects entire lifetime
- [ ] Timeline viewport reflects current selection
- [ ] Dragging viewport updates time range
- [ ] Custom range inputs are clamped to process bounds
- [ ] Warning shown when bounds are enforced
- [ ] Recent ranges show correct anchor context
- [ ] Standard picker unchanged on non-process pages
