# Time Range Picker Calendar Fix Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/930
**Status**: Implemented

## Overview

The "select from calendar" UI in the time range picker dismisses itself after every interaction (selecting a date, changing the time, clicking "Now"). The user can't compose a full date+time — the picker vanishes ("pouf!") after a single action. The root cause is that `CustomRange` closes the DateTimePicker on every `onChange` call, but DateTimePicker fires `onChange` on every interaction.

## Current State

### Flow that causes the bug

1. User opens TimeRangePicker → clicks calendar icon in CustomRange
2. `showFromCalendar` becomes `true` → DateTimePicker renders inline
3. User picks a date in DayPicker (or changes hours/minutes, or clicks "Now")
4. DateTimePicker calls `onChange(newDate)`
5. `handleFromDateSelect` in CustomRange (`CustomRange.tsx:40-46`) runs:
   ```typescript
   const handleFromDateSelect = (date: Date | undefined) => {
     if (date) {
       setFromInput(formatDateTimeLocal(date))
       setShowFromCalendar(false)  // ← immediately hides DateTimePicker
       setError(null)
     }
   }
   ```
6. The entire DateTimePicker section disappears before the user can adjust the time

The same issue affects the "To" calendar (`handleToDateSelect`, line 48-54).

### Secondary issue: z-index conflict

The DateTimePicker's DayPicker popup uses a `fixed inset-0 z-20` overlay (`DateTimePicker.tsx:95`), which is the same z-index as the parent TimeRangePicker's popover content (`index.tsx:211`). Clicking on the DayPicker overlay could dismiss the parent picker.

### Files involved

| File | Role |
|------|------|
| `analytics-web-app/src/components/layout/TimeRangePicker/CustomRange.tsx` | Manages calendar visibility, handles date selection |
| `analytics-web-app/src/components/ui/DateTimePicker.tsx` | Calendar + time input component |

## Design

### Fix 1: Keep DateTimePicker open during editing (CustomRange.tsx)

Change `handleFromDateSelect` and `handleToDateSelect` to **update the input value without closing the calendar**. The user closes the calendar explicitly by clicking the calendar toggle button again.

```typescript
// Before (closes on every change):
const handleFromDateSelect = (date: Date | undefined) => {
  if (date) {
    setFromInput(formatDateTimeLocal(date))
    setShowFromCalendar(false)  // remove this
    setError(null)
  }
}

// After (stays open):
const handleFromDateSelect = (date: Date | undefined) => {
  if (date) {
    setFromInput(formatDateTimeLocal(date))
    setError(null)
  }
}
```

Same change for `handleToDateSelect`.

### Fix 2: Keep DayPicker calendar open after date selection (DateTimePicker.tsx)

Currently `handleDateSelect` closes the DayPicker dropdown after picking a day (`setIsCalendarOpen(false)` at line 27 and 33). Since the DateTimePicker now stays visible, the user should be able to pick a date and then adjust the time without the calendar dropdown closing. Remove `setIsCalendarOpen(false)` from `handleDateSelect`.

The user can close the DayPicker calendar dropdown by:
- Clicking the calendar button again (toggle)
- Clicking the fixed overlay behind the dropdown

```typescript
// Before:
const handleDateSelect = useCallback(
  (date: Date | undefined) => {
    if (!date) {
      if (value) {
        onChange(startOfDay(value))
        setIsCalendarOpen(false)  // remove
      }
      return
    }
    onChange(startOfDay(date))
    setIsCalendarOpen(false)  // remove
  },
  [onChange, value]
)

// After:
const handleDateSelect = useCallback(
  (date: Date | undefined) => {
    if (!date) {
      if (value) {
        onChange(startOfDay(value))
      }
      return
    }
    onChange(startOfDay(date))
  },
  [onChange, value]
)
```

### Fix 3: Fix z-index layering (DateTimePicker.tsx)

Bump the DateTimePicker's DayPicker overlay from `z-20` to `z-30` and its content from `z-30` to `z-40`, so they sit above the parent TimeRangePicker popover content (z-20).

```
Layer stack:
z-10  TimeRangePicker backdrop overlay
z-20  TimeRangePicker popover content
z-30  DayPicker backdrop overlay  (was z-20)
z-40  DayPicker calendar dropdown (was z-30)
```

### Fix 4: Add aria-labels to calendar toggle buttons (CustomRange.tsx)

Both "From" and "To" calendar toggle buttons shared the same `title="Select from calendar"`. Added unique `aria-label` attributes (`"Open start calendar"` / `"Open end calendar"`) for accessibility and test targeting, while keeping the shared `title="Open calendar"` for the tooltip.

## Implementation Steps

1. **`CustomRange.tsx`**: Remove `setShowFromCalendar(false)` from `handleFromDateSelect` and `setShowToCalendar(false)` from `handleToDateSelect`
2. **`CustomRange.tsx`**: Add `aria-label="Open start calendar"` and `aria-label="Open end calendar"` to the toggle buttons, change `title` to `"Open calendar"`
3. **`DateTimePicker.tsx`**: Remove `setIsCalendarOpen(false)` calls from `handleDateSelect`
4. **`DateTimePicker.tsx`**: Change overlay z-index from `z-20` to `z-30`, calendar dropdown from `z-30` to `z-40`
5. **`jest.config.js`**: Add CSS moduleNameMapper so `.css` imports don't break tests
6. **Create test files** for both components

## Files Modified

- `analytics-web-app/src/components/layout/TimeRangePicker/CustomRange.tsx` — stop closing calendar on change, add aria-labels
- `analytics-web-app/src/components/ui/DateTimePicker.tsx` — stop closing DayPicker on selection, fix z-index
- `analytics-web-app/jest.config.js` — add CSS moduleNameMapper

## Files Created

- `analytics-web-app/src/__mocks__/styleMock.js` — empty module for CSS imports
- `analytics-web-app/src/components/ui/__tests__/DateTimePicker.test.tsx` — 6 tests
- `analytics-web-app/src/components/layout/TimeRangePicker/__tests__/CustomRange.test.tsx` — 5 tests

## Trade-offs

**Keeping calendar open vs auto-closing**: The main alternative is to close the DateTimePicker only on specific "final" actions (e.g., only on time input blur). However, this is fragile and inconsistent — the simplest fix is to let the user control when the calendar closes. The calendar toggle button and overlay click-to-dismiss provide clear closing mechanisms.

**Preserving time on date change**: Currently `handleDateSelect` resets time to 00:00 via `startOfDay()`. This is reasonable for the initial date selection but annoying when switching dates after setting a time. A future improvement could preserve the existing time when changing the date, but that's out of scope for this fix.

## Testing Strategy

### Automated Tests

Two test files, one per component. Both use the project's existing Jest + React Testing Library setup. Tests were verified to fail when the bug is reintroduced and pass with the fix applied.

**Jest config prerequisite**: Added a CSS moduleNameMapper to `jest.config.js` so that `.css` imports resolve to an empty module instead of failing:

```javascript
// In moduleNameMapper, add:
'\\.css$': '<rootDir>/src/__mocks__/styleMock.js',
```

#### Test 1: `analytics-web-app/src/components/ui/__tests__/DateTimePicker.test.tsx`

Tests the DateTimePicker component in isolation.

**Test cases (6):**

1. **Calendar opens on button click** — click calendar button, assert DayPicker visible
2. **Calendar stays open after date selection** — select a date, assert DayPicker still visible (regression test)
3. **Time input change calls onChange** — change hours, assert onChange called with updated hours
4. **Quick action buttons call onChange** — click Now/Start of day/End of day, assert onChange called
5. **Calendar closes on overlay click** — click fixed overlay, assert DayPicker gone
6. **Calendar closes on toggle button re-click** — click button again, assert DayPicker gone

#### Test 2: `analytics-web-app/src/components/layout/TimeRangePicker/__tests__/CustomRange.test.tsx`

Tests that CustomRange keeps the DateTimePicker visible after interactions.

**Test cases (5):**

1. **Calendar section appears when button clicked** — click toggle, assert DateTimePicker visible
2. **From calendar stays visible after date selection** — select date, assert still visible (regression test)
3. **To calendar stays visible after date selection** — same for "To" field (regression test)
4. **Calendar closes when toggle clicked again** — click toggle twice, assert gone
5. **From input updates after date selection** — select date, assert input value changed

### Manual Testing

1. Open time range picker → click calendar icon next to "From" input
2. Select a date in the DayPicker calendar → verify the DateTimePicker stays visible
3. Change hours/minutes → verify the DateTimePicker stays visible and the input updates
4. Click "Now", "Start of day", "End of day" → verify DateTimePicker stays visible
5. Click the calendar toggle button → verify the DateTimePicker closes
6. Repeat steps 1-5 for the "To" calendar
7. Click "Apply time range" → verify the range is applied and the picker closes
8. Verify the DayPicker calendar dropdown doesn't cause the parent popover to close
