# Notebook Time Range Selection Plan

## Overview

Add drag-to-zoom time range selection on chart cells in notebooks, enabling users to navigate time by selecting regions on any time-synced cell. This brings the Performance Analysis screen's coordinated time navigation to notebooks.

## Related Issue

GitHub Issue: [#765 - Add time range selection to notebook charts](https://github.com/madesroches/micromegas/issues/765)

## Current State Analysis

### Existing Infrastructure

The codebase already has most of the infrastructure needed:

1. **XYChart Component** (`src/components/XYChart.tsx`)
   - Uses uPlot with drag-to-select enabled for time axis mode (line 447: `drag: { x: xAxisMode === 'time' }`)
   - Already supports `onTimeRangeSelect?: (from: Date, to: Date) => void` prop (line 36)
   - Selection logic in `setSelect` hook converts pixel positions to timestamps (lines 471-490)

2. **PropertyTimeline Component** (`src/components/PropertyTimeline.tsx`)
   - Has its own drag-to-select implementation (lines 104-149)
   - Supports `onTimeRangeSelect?: (from: Date, to: Date) => void` prop (line 18)
   - Minimum 5-pixel drag threshold
   - Already shows crosshair cursor when callback is provided (line 266)

3. **NotebookRenderer** (`src/lib/screen-renderers/NotebookRenderer.tsx`)
   - Has access to `onTimeRangeChange: (from: string, to: string) => void` via `ScreenRendererProps`
   - Currently does NOT destructure or use `onTimeRangeChange`
   - Cells execute with `timeRange: { begin, end }` in execution context

4. **ScreenRendererProps** (`src/lib/screen-renderers/index.ts`)
   - Defines `onTimeRangeChange: (from: string, to: string) => void` (line 32)
   - Updates URL params which triggers re-execution of all cells

### What's Missing

The callbacks exist but aren't wired together:
- NotebookRenderer doesn't destructure `onTimeRangeChange` from props
- NotebookRenderer doesn't pass `onTimeRangeSelect` to chart/property timeline cells
- ChartCell renders XYChart but doesn't receive or pass the callback
- PropertyTimelineCell renders PropertyTimeline but doesn't receive or pass the callback

## Implementation Plan

### Phase 1: Wire Up Time Range Selection Callbacks

#### 1.1 Update CellRendererProps Type

**File**: `src/lib/screen-renderers/cell-registry.ts`

Add `onTimeRangeSelect` to the `CellRendererProps` interface (around line 11):

```typescript
export interface CellRendererProps {
  // ... existing props (name, sql, options, data, status, etc.)

  /** Callback for drag-to-zoom time selection (chart and property timeline cells) */
  onTimeRangeSelect?: (from: Date, to: Date) => void
}
```

#### 1.2 Update ChartCell to Pass Callback to XYChart

**File**: `src/lib/screen-renderers/cells/ChartCell.tsx`

The ChartCell component already receives all `CellRendererProps`. Add `onTimeRangeSelect` to destructuring and pass to XYChart:

```typescript
export function ChartCell({
  data,
  status,
  options,
  onOptionsChange,
  variables,
  timeRange,
  onTimeRangeSelect  // ADD THIS
}: CellRendererProps) {
  // ... existing code ...

  return (
    <div className="h-full">
      <XYChart
        data={chartData}
        xAxisMode={xAxisMode}
        xLabels={xLabels}
        xColumnName={xColumnName}
        yColumnName={yColumnName}
        scaleMode={(resolvedOptions?.scale_mode as ScaleMode) ?? 'p99'}
        onScaleModeChange={handleScaleModeChange}
        chartType={(resolvedOptions?.chart_type as ChartType) ?? 'line'}
        onChartTypeChange={handleChartTypeChange}
        unit={(resolvedOptions?.unit as string) ?? undefined}
        onTimeRangeSelect={onTimeRangeSelect}  // ADD THIS
      />
    </div>
  )
}
```

Note: XYChart internally only enables drag selection when `xAxisMode === 'time'`, so no conditional logic needed here.

#### 1.3 Update PropertyTimelineCell to Pass Callback

**File**: `src/lib/screen-renderers/cells/PropertyTimelineCell.tsx`

Add `onTimeRangeSelect` to destructuring and pass to PropertyTimeline:

```typescript
export function PropertyTimelineCell({
  data,
  status,
  options,
  onOptionsChange,
  timeRange,
  onTimeRangeSelect,  // ADD THIS
}: CellRendererProps) {
  // ... existing code ...

  return (
    <div className="h-full flex flex-col">
      <ParseErrorWarning errors={errors} className="mb-2" />
      <PropertyTimeline
        properties={timelines}
        availableKeys={availableKeys}
        selectedKeys={selectedKeys}
        timeRange={{ from: timeRangeMs.begin, to: timeRangeMs.end }}
        onAddProperty={handleAddProperty}
        onRemoveProperty={handleRemoveProperty}
        showTimeAxis={true}
        onTimeRangeSelect={onTimeRangeSelect}  // ADD THIS
      />
    </div>
  )
}
```

#### 1.4 Update NotebookRenderer to Create and Pass Callback

**File**: `src/lib/screen-renderers/NotebookRenderer.tsx`

**Step 1**: Destructure `onTimeRangeChange` from props (around line 169):

```typescript
export function NotebookRenderer({
  config,
  onConfigChange,
  savedConfig,
  setHasUnsavedChanges,
  timeRange,
  rawTimeRange,
  onTimeRangeChange,  // ADD THIS
  onSave,
  isSaving,
  hasUnsavedChanges,
  onSaveAs,
  saveError,
  refreshTrigger,
}: ScreenRendererProps) {
```

**Step 2**: Create the handler that converts Date to ISO strings (add after the hooks, around line 280):

```typescript
// Handle time range selection from charts (drag-to-zoom)
const handleTimeRangeSelect = useCallback((from: Date, to: Date) => {
  onTimeRangeChange(from.toISOString(), to.toISOString())
}, [onTimeRangeChange])
```

**Step 3**: Pass the callback in the `renderCell` function (around line 505, alongside other callbacks):

```typescript
<CellRenderer
  name={cell.name}
  data={state.data}
  status={state.status}
  error={state.error}
  timeRange={timeRange}
  variables={availableVariables}
  isEditing={selectedCellIndex === index}
  onRun={() => executeCell(index)}
  onSqlChange={(sql) => updateCell(index, { sql })}
  onOptionsChange={(options) => updateCell(index, { options })}
  onContentChange={(content) => updateCell(index, { content })}
  onTimeRangeSelect={handleTimeRangeSelect}  // ADD THIS
  value={cell.type === 'variable' ? variableValues[cell.name] : undefined}
  onValueChange={cell.type === 'variable' ? (value) => setVariableValue(cell.name, value) : undefined}
  {...rendererProps}
/>
```

Note: All cell types receive this prop, but only ChartCell and PropertyTimelineCell use it. Other cells simply ignore it.

### Phase 2: Visual Feedback (Already Implemented)

Both XYChart and PropertyTimeline already have visual feedback:
- **XYChart**: Selection overlay with border styling (uPlot's built-in selection)
- **PropertyTimeline**: Crosshair cursor when `onTimeRangeSelect` is provided (line 266), selection overlay (lines 274-284)

No additional work needed for Phase 2.

### Phase 3: Time Sync Configuration (Future Enhancement)

Defer until there's a real need. The default behavior (enabled for time-axis charts) is sensible.

If needed later, add opt-out via cell options:

```typescript
interface ChartCellOptions {
  // ... existing options
  timeSync?: boolean  // default: true for time-axis charts
}
```

### Phase 4: Reset/History (Future Enhancement)

Track time range changes for back/forward navigation. Defer to future work.

## Files to Modify

| File | Changes |
|------|---------|
| `src/lib/screen-renderers/cell-registry.ts` | Add `onTimeRangeSelect` to `CellRendererProps` type |
| `src/lib/screen-renderers/cells/ChartCell.tsx` | Destructure `onTimeRangeSelect`, pass to XYChart |
| `src/lib/screen-renderers/cells/PropertyTimelineCell.tsx` | Destructure `onTimeRangeSelect`, pass to PropertyTimeline |
| `src/lib/screen-renderers/NotebookRenderer.tsx` | Destructure `onTimeRangeChange`, create handler, pass to CellRenderer |

## Testing Strategy

### Manual Testing

1. Create a notebook with a chart cell using time-axis data (e.g., `SELECT time, value FROM ...`)
2. Drag to select a time region on the chart
3. Verify URL updates with new time range (check `from` and `to` query params)
4. Verify all cells re-execute with new time range
5. Test with PropertyTimeline cells
6. Test that non-time-axis charts (categorical, numeric X) don't show selection behavior

### Unit Tests

1. Test `handleTimeRangeSelect` converts Date objects to ISO strings correctly
2. Test that chart cells with `xAxisMode !== 'time'` don't trigger selection (XYChart handles this)

### Integration Tests

1. Test end-to-end flow: drag → URL update → re-execution
2. Test multiple time-synced cells update together
3. Test that drag on PropertyTimeline updates all cells

## Implementation Order

1. **cell-registry.ts** - Add type definition
2. **ChartCell.tsx** - Wire up callback
3. **PropertyTimelineCell.tsx** - Wire up callback
4. **NotebookRenderer.tsx** - Create handler and pass to cells
5. **Manual testing** - Verify end-to-end flow

## Estimated Scope

- Phase 1: ~20-30 lines changed across 4 files
- Phase 2: Already implemented (0 lines)
- Phase 3: Deferred
- Phase 4: Deferred

The core implementation leverages existing infrastructure and is straightforward since all the underlying components already support the necessary callbacks.

## Resolved Questions

1. **Should time sync be opt-in or opt-out per cell?**
   - Decision: Defer configuration option. Default behavior (enabled for time-axis charts, handled by XYChart internally) is sufficient.

2. **Should we show visual indication that a cell supports drag-to-zoom?**
   - Decision: Already implemented - PropertyTimeline shows crosshair cursor, XYChart has built-in selection styling.

3. **Priority of history/reset features?**
   - Decision: Defer to future enhancement, not blocking for initial implementation.
