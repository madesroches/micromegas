# Time Range Picker Calendar Fix Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/930

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

## Implementation Steps

1. **`CustomRange.tsx`**: Remove `setShowFromCalendar(false)` from `handleFromDateSelect` and `setShowToCalendar(false)` from `handleToDateSelect`
2. **`CustomRange.tsx`**: Change the "To" calendar toggle button title from `"Select from calendar"` to `"Select to calendar"` (both buttons currently share the same title, which breaks test queries)
3. **`DateTimePicker.tsx`**: Remove `setIsCalendarOpen(false)` calls from `handleDateSelect`
4. **`DateTimePicker.tsx`**: Change overlay z-index from `z-20` to `z-30`, calendar dropdown from `z-30` to `z-40`

## Files to Modify

- `analytics-web-app/src/components/layout/TimeRangePicker/CustomRange.tsx` — stop closing calendar on change, fix duplicate button title
- `analytics-web-app/src/components/ui/DateTimePicker.tsx` — stop closing DayPicker on selection, fix z-index
- `analytics-web-app/jest.config.js` — add CSS moduleNameMapper so `.css` imports don't break tests

## Trade-offs

**Keeping calendar open vs auto-closing**: The main alternative is to close the DateTimePicker only on specific "final" actions (e.g., only on time input blur). However, this is fragile and inconsistent — the simplest fix is to let the user control when the calendar closes. The calendar toggle button and overlay click-to-dismiss provide clear closing mechanisms.

**Preserving time on date change**: Currently `handleDateSelect` resets time to 00:00 via `startOfDay()`. This is reasonable for the initial date selection but annoying when switching dates after setting a time. A future improvement could preserve the existing time when changing the date, but that's out of scope for this fix.

## Testing Strategy

### Automated Tests

Two test files, one per component. Both use the project's existing Jest + React Testing Library setup.

**Jest config prerequisite**: Add a CSS moduleNameMapper to `jest.config.js` so that `.css` imports resolve to an empty module instead of failing:

```javascript
// In moduleNameMapper, add:
'\\.css$': '<rootDir>/src/__mocks__/styleMock.js',
```

Create `analytics-web-app/src/__mocks__/styleMock.js`:
```javascript
module.exports = {}
```

This handles `react-day-picker/style.css`, `./DateTimePicker.css`, and any future CSS imports globally — no per-test CSS mocks needed.

#### Test 1: `analytics-web-app/src/components/ui/__tests__/DateTimePicker.test.tsx`

Tests the DateTimePicker component in isolation. The `react-day-picker` library (ESM) needs mocking; CSS imports are handled by the global moduleNameMapper above.

**Mocks required:**
- `react-day-picker` — render a simple button that calls `onSelect` when clicked
- `lucide-react` — simple span stubs (existing pattern from `CellContainer.test.tsx`)
- `date-fns` — let it run (CJS v2, works in Jest)

**Test cases:**

1. **Calendar opens on button click**
   - Render with a value, click the calendar button
   - Assert the DayPicker mock is visible

2. **Calendar stays open after date selection** (validates fix 2)
   - Open calendar, click the mocked DayPicker day button
   - Assert `onChange` was called
   - Assert the DayPicker mock is **still** in the document

3. **Time input change calls onChange without side effects**
   - Render with a value, change the hours input via `fireEvent.change`
   - Assert `onChange` was called with updated hours
   - (No calendar open/close involved — just a sanity check)

4. **Quick action buttons call onChange**
   - Click "Now" → assert `onChange` called
   - Click "Start of day" → assert `onChange` called
   - Click "End of day" → assert `onChange` called

5. **Calendar closes on overlay click**
   - Open calendar, click the fixed overlay
   - Assert the DayPicker mock is no longer visible

6. **Calendar closes on toggle button re-click**
   - Open calendar, click the calendar button again
   - Assert the DayPicker mock is no longer visible

**react-day-picker mock shape:**
```typescript
jest.mock('react-day-picker', () => ({
  DayPicker: ({ onSelect, selected }: { onSelect?: (date: Date) => void, selected?: Date }) => (
    <div data-testid="day-picker">
      <button data-testid="day-picker-day" onClick={() => onSelect?.(new Date(2026, 2, 15))}>
        15
      </button>
      {selected && <span data-testid="day-picker-selected">{selected.toISOString()}</span>}
    </div>
  ),
}))
```

#### Test 2: `analytics-web-app/src/components/layout/TimeRangePicker/__tests__/CustomRange.test.tsx`

Tests that CustomRange keeps the DateTimePicker visible after interactions.

**Mocks required:**
- `@/components/ui/DateTimePicker` — render a visible div with a button that calls `onChange` when clicked (simulates any DateTimePicker interaction)
- `@/lib/time-range` — provide real `isValidTimeExpression`, `parseRelativeTime`, `formatDateTimeLocal` (they're pure functions, work in Jest)
- `lucide-react` — span stubs

**DateTimePicker mock shape:**
```typescript
jest.mock('@/components/ui/DateTimePicker', () => ({
  DateTimePicker: ({ value, onChange }: { value?: Date, onChange: (d: Date) => void }) => (
    <div data-testid="date-time-picker">
      <button data-testid="mock-date-select" onClick={() => onChange(new Date(2026, 2, 15))}>
        Select Date
      </button>
      {value && <span data-testid="picker-value">{value.toISOString()}</span>}
    </div>
  ),
}))
```

**Test cases:**

1. **Calendar section appears when calendar button clicked**
   - Render CustomRange with `from="now-1h"` and `to="now"`
   - Click the "From" calendar toggle button (by `title="Select from calendar"`)
   - Assert `data-testid="date-time-picker"` is in the document

2. **Calendar section stays visible after date selection** (validates fix 1)
   - Open the "From" calendar
   - Click the mock "Select Date" button (triggers `onChange`)
   - Assert `data-testid="date-time-picker"` is **still** in the document
   - Assert the "From" text input updated with the formatted date

3. **Calendar section stays visible for "To" field too**
   - Same as test 2 but for the "To" calendar toggle (by `title="Select to calendar"`)

4. **Calendar closes when toggle button clicked again**
   - Open the "From" calendar, click the toggle button again
   - Assert `data-testid="date-time-picker"` is **not** in the document

5. **Apply button calls onApply with current input values**
   - Open calendar, select a date (updates the input), click "Apply time range"
   - Assert `onApply` called with the formatted date strings

### Manual Testing

1. Open time range picker → click calendar icon next to "From" input
2. Select a date in the DayPicker calendar → verify the DateTimePicker stays visible
3. Change hours/minutes → verify the DateTimePicker stays visible and the input updates
4. Click "Now", "Start of day", "End of day" → verify DateTimePicker stays visible
5. Click the calendar toggle button → verify the DateTimePicker closes
6. Repeat steps 1-5 for the "To" calendar
7. Click "Apply time range" → verify the range is applied and the picker closes
8. Verify the DayPicker calendar dropdown doesn't cause the parent popover to close

## Implementation Order

1. Write tests first (they will fail, confirming the bug)
2. Apply the 3 fixes
3. Run tests again (they should pass, confirming the fix)

## Files to Create

- `analytics-web-app/src/__mocks__/styleMock.js` — empty module for CSS imports
- `analytics-web-app/src/components/ui/__tests__/DateTimePicker.test.tsx`
- `analytics-web-app/src/components/layout/TimeRangePicker/__tests__/CustomRange.test.tsx`

## Open Questions

None — the fix is straightforward and scoped to the reported bug.
