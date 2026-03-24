# Flamegraph Initial Time Range Plan

## Overview

Add optional `initialFrom` and `initialTo` properties to the flamegraph cell options, allowing users to specify an initial view time range. Values support macros (`$from`, `$to`, `$variable`, `$cell[N].col`, etc.) and relative expressions (`now-1h`), so the flamegraph can open pre-zoomed to a region of interest instead of always showing the full data range.

## Current State

The flamegraph cell (`FlameGraphCell.tsx`) always initializes its view to the full data range:

```typescript
// FlameGraphCell.tsx:472-473
viewMinTime: index.timeRange.min,
viewMaxTime: index.timeRange.max,
```

This is set in the `FlameGraphView` component's `useEffect` (line 715-716) and in the `stateRef` initializer (line 472-473). Double-click resets to the same full range (line 982-983).

The cell uses `QueryCellConfig` with a generic `options?: Record<string, unknown>`. The `getRendererProps` already passes `options` through to `CellRendererProps`, but the renderer currently ignores them.

The `CellRendererProps` already provides `variables`, `timeRange`, `cellResults`, and `cellSelections` — everything needed to resolve macros at render time.

## Design

### Options Schema

Add two optional string fields to the flamegraph options:

```typescript
// In the cell options object:
{
  initialFrom?: string  // e.g., "$from", "$start_time", "now-1h", "$spans[0].begin"
  initialTo?: string    // e.g., "$to", "$end_time", "now", "$spans[0].end"
}
```

### Macro Resolution Flow

Resolve macros in the `FlameGraphCell` renderer component (not in `execute` or `getRendererProps`) since the renderer already receives all macro context via `CellRendererProps`:

1. Read `initialFrom`/`initialTo` from `options`
2. If empty or absent, treat as unset (no initial range for that bound)
3. Run each through `substituteMacros()` to resolve `$from`, `$to`, variables, cell references
4. Parse the resolved string via `parseRelativeTime()` wrapped in try-catch — if it throws, surface the error to the user (see error handling below)
5. Convert to milliseconds via `Date.getTime()`
6. Pass to `FlameGraphView` as optional prop

**Error handling:** `parseRelativeTime()` throws on invalid values. The `resolveInitialTimeRange` helper must catch these exceptions. Invalid values are user errors (typo, wrong macro) and should be surfaced — return an error string alongside the resolved range so the renderer can display it. Empty/absent values are not errors and produce no range constraint.

Using `substituteMacros()` for resolution is fine here — it does SQL single-quote escaping but timestamps don't contain quotes, so the result is identical to the raw value.

### FlameGraphView Changes

Add an optional `initialTimeRange` prop:

```typescript
interface FlameGraphViewProps {
  index: FlameIndex
  onTimeRangeSelect?: (from: Date, to: Date) => void
  initialTimeRange?: { min: number; max: number }  // milliseconds
}
```

Do **not** modify the main setup/teardown `useEffect` (`[index, requestRender]`) — it creates/disposes Three.js resources. Instead, add a **separate** `useEffect` keyed on `[initialTimeRange, requestRender]` that applies the initial view bounds when provided. The main effect keeps initializing to `index.timeRange` as the default; the separate effect overrides it when `initialTimeRange` is set. Because `requestRender` changes whenever `index` changes, the separate effect fires on every re-execution, ensuring the initial range is always reapplied after new data arrives.

Double-click reset keeps resetting to the **full data range** (not initial range), so the user can always see all data.

### Edge Cases

- **Only one bound set**: pair with the corresponding data range bound (e.g., `initialFrom` set but not `initialTo` → use data max for max)
- **Unresolved macros** (unknown variable): `substituteMacros()` leaves unresolved macros as-is (e.g., `$foo` stays as `$foo`), then `parseRelativeTime("$foo")` throws `"Invalid time value: $foo"`. The `resolveInitialTimeRange` helper surfaces this as an error banner (e.g., "Invalid initial from: Invalid time value: $foo")
- **Invalid date after resolution**: show error message (e.g., "Invalid initial from: not a valid date")
- **Initial range outside data range**: allowed — view shows empty space, user can double-click to reset to full range
- **Empty strings**: treated as unset (no error, full data range used for that bound)

### Editor UI

Add two optional text inputs to `FlameGraphCellEditor` below the SQL editor, with a collapsible "View Options" section:

```
▶ View Options
  Initial From: [________________]
  Initial To:   [________________]
```

Show the `AvailableVariablesPanel` below to remind users of available macros.

## Implementation Steps

### Step 1: Add initial time range resolution in FlameGraphCell renderer

**File:** `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx`

- Destructure `options`, `variables`, `timeRange`, `cellResults`, `cellSelections` from props in `FlameGraphCell`
- Add a helper function `resolveInitialTimeRange` that:
  - Reads `options?.initialFrom` and `options?.initialTo` (if non-empty strings)
  - Runs each through `substituteMacros(value, variables, timeRange, cellResults, cellSelections)`
  - Wraps `parseRelativeTime()` in try-catch — on failure, collect error message
  - On success, converts to ms via `Date.getTime()`
  - Returns `{ range?: { min: number; max: number }; error?: string }`
- If error is returned, render it as an error banner above the flame graph (same red style as schema validation errors)
- Wrap the call in `useMemo` keyed on `[options, variables, timeRange, cellResults, cellSelections]` so `initialTimeRange` has a stable object reference (avoids re-triggering effects in FlameGraphView)
- Pass the resolved range to `FlameGraphView` as `initialTimeRange`

### Step 2: Update FlameGraphView to accept initial time range

**File:** `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx`

- Add `initialTimeRange?: { min: number; max: number }` to `FlameGraphViewProps`
- Do **NOT** add `initialTimeRange` to the main setup/teardown `useEffect` dependency array (`[index, requestRender]`) — that effect creates/disposes the Three.js WebGLRenderer, Camera, Scene, and Mesh. Changing its deps would cause full re-initialization on every range change.
- Instead, add a **separate** `useEffect` keyed on `[initialTimeRange, requestRender]` that applies the initial view bounds:
  ```typescript
  useEffect(() => {
    if (!initialTimeRange) return
    const s = stateRef.current
    s.viewMinTime = initialTimeRange.min
    s.viewMaxTime = initialTimeRange.max
    requestRender()
  }, [initialTimeRange, requestRender])
  ```
- In the main setup `useEffect` (line 700+), keep the existing `index.timeRange` initialization as the default — the separate effect overrides it when `initialTimeRange` is provided.
- Keep double-click reset using `index.timeRange` (full data range)

### Step 3: Add editor UI fields

**File:** `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx`

- Add a collapsible "View Options" section in `FlameGraphCellEditor`
- Two text inputs for `initialFrom` and `initialTo`
- Read/write via `config.options.initialFrom` / `config.options.initialTo`
- Show placeholder text hinting at macro support (e.g., `$from, now-1h, or variable`)

### Step 4: Update default config

**File:** `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx`

- No change needed — `options` defaults to `{}` and missing keys mean "unset" (full range)

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx` — all changes are in this one file

## Trade-offs

**Resolve in renderer vs. in execute:**
Resolving macros in the renderer is simpler (no CellState changes, no new pipeline) and the renderer already has all the context. The downside is a small amount of work on each render, but `substituteMacros` is cheap and only runs when `options` change (via `useMemo`).

**Separate fields vs. single expression:**
Two separate fields (`initialFrom`/`initialTo`) are clearer for the user and match the `$from`/`$to` pattern. A single expression would be more flexible but harder to validate.

**Double-click reset behavior:**
Resets to full data range rather than initial range. This is more useful because the initial range is a starting point, and users need a way to see all data. The initial range is reapplied when the cell re-executes.

## Testing Strategy

- **Manual**: Set `initialFrom`/`initialTo` in the editor with various values (`$from`, `now-1h`, variable references, cell result references) and verify the flamegraph opens zoomed to the expected range
- **Error display**: Verify that invalid values (typo, unresolved macro) show an error message in the cell instead of silently falling back
- **Edge cases**: Test with only one bound set, empty strings, ranges outside data bounds
- **Unit test**: Add a test for the `resolveInitialTimeRange` helper function covering: valid macros, valid relative times, invalid strings (expect error), empty strings (expect no error, no range)

## Open Questions

None — the design follows established patterns for options and macro resolution.
