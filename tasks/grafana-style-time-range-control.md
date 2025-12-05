# Grafana-Style Time Range Control for Analytics Web App

## Overview

Replace the current simple dropdown time range selector with a comprehensive Grafana-style time range picker featuring quick presets, custom relative time input, absolute date/time selection with calendar, and recent history.

> **Note**: Auto-refresh is a separate feature and not part of this plan.

## Current State

**Location**: `analytics-web-app/src/components/layout/TimeRangeSelector.tsx`

**Current features**:
- Simple dropdown with 8 preset relative time ranges
- URL-based state management (`?from=now-24h&to=now`)
- Supports relative (`now-1h`) and absolute (ISO date) formats
- No calendar picker, no custom input, no history, no auto-refresh

## Target State

A multi-panel time range picker similar to Grafana with:

```
┌───────────────────────────────────────┐
│  [Clock Icon] Last 24 hours  [▼]      │
└───────────────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────────────────────────────┐
│ ┌─────────────────┐ ┌───────────────────────────────────────┐   │
│ │ Quick ranges    │ │ From: [now-24h          ] [📅]        │   │
│ │ ─────────────── │ │ To:   [now              ] [📅]        │   │
│ │ Last 5 minutes  │ │                                       │   │
│ │ Last 15 minutes │ │ [Apply time range]                    │   │
│ │ Last 30 minutes │ └───────────────────────────────────────┘   │
│ │ Last 1 hour     │ ┌───────────────────────────────────────┐   │
│ │ Last 3 hours    │ │ Recently used:                        │   │
│ │ Last 6 hours    │ │ • Last 1 hour                         │   │
│ │ Last 12 hours   │ │ • Dec 4, 10:00 - Dec 5, 10:00         │   │
│ │ Last 24 hours ✓ │ │ • Last 7 days                         │   │
│ │ Last 2 days     │ └───────────────────────────────────────┘   │
│ │ Last 7 days     │                                             │
│ │ Last 30 days    │                                             │
│ │ Last 90 days    │                                             │
│ └─────────────────┘                                             │
└─────────────────────────────────────────────────────────────────┘
```

## Implementation Plan

### Phase 1: Extend Time Range Utilities

**File**: `src/lib/time-range.ts`

1. **Add more preset options**:
   ```typescript
   export const TIME_RANGE_PRESETS = [
     { label: 'Last 5 minutes', value: 'now-5m' },
     { label: 'Last 15 minutes', value: 'now-15m' },
     { label: 'Last 30 minutes', value: 'now-30m' },
     { label: 'Last 1 hour', value: 'now-1h' },
     { label: 'Last 3 hours', value: 'now-3h' },
     { label: 'Last 6 hours', value: 'now-6h' },
     { label: 'Last 12 hours', value: 'now-12h' },
     { label: 'Last 24 hours', value: 'now-24h' },
     { label: 'Last 2 days', value: 'now-2d' },
     { label: 'Last 7 days', value: 'now-7d' },
     { label: 'Last 30 days', value: 'now-30d' },
     { label: 'Last 90 days', value: 'now-90d' },
   ]
   ```

2. **Add support for more time units**:
   - Extend regex to support weeks (`w`), seconds (`s`)
   - Compound expressions (`now-1h30m`) deferred to Phase 7

3. **Add validation function**:
   ```typescript
   export function isValidTimeExpression(value: string): boolean
   ```

4. **Add formatting for relative expressions**:
   ```typescript
   export function formatRelativeTime(value: string): string
   // "now-1h" -> "Last 1 hour"
   // "now-90m" -> "Last 90 minutes"
   ```

### Phase 2: Recent Time Ranges (localStorage)

**New file**: `src/lib/time-range-history.ts`

```typescript
const HISTORY_KEY = 'micromegas-time-range-history'
const MAX_HISTORY = 5  // May adjust based on UI space

interface TimeRangeHistoryEntry {
  from: string
  to: string
  label: string
  timestamp: number
}

export function saveTimeRange(from: string, to: string, label: string): void
export function getRecentTimeRanges(): TimeRangeHistoryEntry[]
export function clearTimeRangeHistory(): void
```

### Phase 3: Calendar/DateTime Picker Component

**New file**: `src/components/ui/DateTimePicker.tsx`

Options for implementation:
1. **Use existing Radix primitives** - Build custom with Radix Popover + custom calendar grid
2. **Add react-day-picker** - Lightweight, accessible calendar component
3. **Add date-fns** - For date manipulation (already common in JS projects)

Recommended: Add `react-day-picker` + `date-fns`

```bash
yarn add react-day-picker date-fns
```

Component features:
- Calendar grid for date selection
- Time input (hours:minutes)
- Display in local timezone, convert to RFC3339 (UTC) for API
- Quick buttons: "Now", "Start of day", "End of day"

### Phase 4: New Time Range Picker Component

**Refactor**: `src/components/layout/TimeRangeSelector.tsx` -> `TimeRangePicker.tsx`

Structure:
```
TimeRangePicker/
├── index.tsx              # Main component with popover
├── QuickRanges.tsx        # Left panel with preset list
├── CustomRange.tsx        # Right panel with from/to inputs
├── RecentRanges.tsx       # Recent history section
└── types.ts               # Shared types
```

**Main component features**:
- Click button to open popover (not dropdown)
- Two-column layout inside popover
- Left: Scrollable list of quick presets with search/filter
- Right: Custom range inputs with calendar pickers
- Bottom: Recent history section

### Phase 5: Update Hook and Integration

**Update**: `src/hooks/useTimeRange.ts`

Add:
```typescript
export interface UseTimeRangeReturn {
  // ... existing
  setAbsoluteRange: (from: Date, to: Date) => void
}
```

**Update**: `src/components/layout/Header.tsx`
- Replace `TimeRangeSelector` with new `TimeRangePicker`

### Phase 6: Keyboard Shortcuts (Optional Enhancement)

- `t` to open time picker
- `Escape` to close
- Arrow keys to navigate presets
- `Enter` to select

## File Changes Summary

### New Files
- `src/lib/time-range-history.ts` - localStorage history management
- `src/components/ui/DateTimePicker.tsx` - Calendar/time picker
- `src/components/layout/TimeRangePicker/index.tsx` - Main picker
- `src/components/layout/TimeRangePicker/QuickRanges.tsx`
- `src/components/layout/TimeRangePicker/CustomRange.tsx`
- `src/components/layout/TimeRangePicker/RecentRanges.tsx`

### Modified Files
- `src/lib/time-range.ts` - Extended presets and utilities
- `src/hooks/useTimeRange.ts` - Add absolute range support
- `src/components/layout/Header.tsx` - Use new picker
- `package.json` - Add react-day-picker, date-fns

### Deleted Files
- `src/components/layout/TimeRangeSelector.tsx` - Replaced by TimeRangePicker

## Dependencies to Add

```json
{
  "react-day-picker": "^8.10.0",
  "date-fns": "^3.0.0"
}
```

## Testing Considerations

1. Unit tests for time-range utilities (parsing, validation)
2. Unit tests for history localStorage operations
3. Component tests for picker interactions
4. E2E test: Select preset -> verify URL updates -> verify data refreshes
5. E2E test: Select absolute range from calendar -> verify API call

## Accessibility

- Keyboard navigation throughout
- ARIA labels on all interactive elements
- Focus management when popover opens/closes
- Screen reader announcements for selection changes

## Implementation Order

1. [ ] Phase 1: Extend time range utilities
2. [ ] Phase 2: Recent time ranges (localStorage)
3. [ ] Phase 3: Calendar/DateTime picker (add dependencies first)
4. [ ] Phase 4: New TimeRangePicker component
5. [ ] Phase 5: Update hook and Header integration
6. [ ] Phase 6: Keyboard shortcuts (optional)
7. [ ] Phase 7: Compound time expressions (optional) - e.g., `now-1h30m`

## Decisions

1. **Timezone**: Display in local timezone in the UI. Backend always receives RFC3339 (UTC). Future enhancement: add local/UTC display toggle in frontend.
2. **Compound expressions**: Support `now-1h30m` style expressions, but as a low-priority enhancement (Phase 7).
3. **Recent ranges**: Store 5 entries. May adjust based on UI space.
