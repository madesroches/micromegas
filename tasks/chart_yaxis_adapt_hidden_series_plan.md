# Charts: Y-axis Scale Should Adapt When Series Are Hidden

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/836

## Overview

When hiding a series in a multi-series chart, the Y-axis scale still accounts for the hidden series' values, making smaller series hard to inspect. The fix is to filter out hidden series from the `unitScaleInfo` computation so the Y-axis range recalculates based only on visible series.

## Current State

**File:** `analytics-web-app/src/components/XYChart.tsx`

The chart has two relevant data flows:

1. **Stats computation** (line 213): `allSeriesStats` computes per-series `{min, max, avg, p99}` from all series data.

2. **Unit scale info** (lines 220-241): `unitScaleInfo` groups series by unit and takes the max `p99` and `max` across ALL series in each unit group — regardless of visibility:
   ```typescript
   info.p99 = Math.max(info.p99, allSeriesStats[i].p99)
   info.max = Math.max(info.max, allSeriesStats[i].max)
   ```

3. **Scale range** (lines 593-599): The Y-axis range function uses `scaleP99`/`scaleMax` from `unitScaleInfo`, which includes hidden series values.

4. **Series visibility** (line 644): Hidden series get `show: false` in uPlot config, but the scale was already computed from all series.

5. **Adaptive unit info** (lines 574-585): Uses `unitScaleInfo` to compute conversion factors for time/size unit formatting on axes.

The `unitScaleInfo` `useMemo` depends on `[normalizedSeries, allSeriesStats]` but not `seriesVisibility`. Even though `seriesVisibility` is in the chart `useEffect` deps (line 830), the `unitScaleInfo` memo is stale with respect to visibility changes.

## Design

Add `seriesVisibility` as a dependency of `unitScaleInfo` and skip hidden series when computing the per-unit p99/max aggregations.

When all series for a given unit are hidden, keep the scale at a sensible default (e.g., the original values) rather than collapsing to zero.

### Changes to `unitScaleInfo` computation (lines 220-241)

```typescript
const unitScaleInfo = useMemo(() => {
  const unitMap = new Map<string, { seriesIndices: number[]; p99: number; max: number; hasVisible: boolean }>()
  for (let i = 0; i < normalizedSeries.length; i++) {
    const u = normalizedSeries[i].unit || ''
    if (!unitMap.has(u)) {
      unitMap.set(u, { seriesIndices: [], p99: 0, max: 0, hasVisible: false })
    }
    const info = unitMap.get(u)!
    info.seriesIndices.push(i)
    // Only include visible series in scale calculations
    const isVisible = seriesVisibility ? seriesVisibility[i] : true
    if (isVisible) {
      info.hasVisible = true
      info.p99 = Math.max(info.p99, allSeriesStats[i].p99)
      info.max = Math.max(info.max, allSeriesStats[i].max)
    }
  }

  // Fallback: if all series for a unit are hidden, use all-series stats to avoid zero scale
  for (const [, info] of unitMap) {
    if (!info.hasVisible) {
      for (const idx of info.seriesIndices) {
        info.p99 = Math.max(info.p99, allSeriesStats[idx].p99)
        info.max = Math.max(info.max, allSeriesStats[idx].max)
      }
    }
  }

  const entries = [...unitMap.entries()]
  return entries.map(([unitName, info], idx) => ({
    unitName,
    scaleName: unitName || 'y',
    side: idx === 0 ? 1 : idx === 1 ? 3 : idx % 2 === 0 ? 1 : 3,
    ...info,
  }))
}, [normalizedSeries, allSeriesStats, seriesVisibility])
```

The single-series path (lines 708+) is unaffected since there's only one series and no legend toggle.

## Implementation Steps

1. **Update `unitScaleInfo` useMemo** in `XYChart.tsx` (lines 220-241):
   - Add `seriesVisibility` to the dependency array
   - Skip hidden series when computing `p99`/`max` aggregates
   - Add fallback for when all series in a unit group are hidden

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/src/components/XYChart.tsx` | Filter hidden series in `unitScaleInfo` computation |

## Trade-offs

**Chosen approach: Filter at `unitScaleInfo` level**
- Minimal change — single `useMemo` update
- The scale info is the single source of truth for Y-axis ranges, so filtering here fixes both the scale and the adaptive unit formatting

**Alternative considered: Filter at the `scales` range function level**
- Would require the range function closures to have access to visibility state and raw per-series stats
- More complex, more closures capturing mutable state

**Alternative considered: Recompute `allSeriesStats` filtering hidden series**
- Heavier computation (re-sorting arrays) for no benefit — we already have per-series stats, just need to skip hidden ones during aggregation

## Testing Strategy

1. **Manual testing**: Open a multi-series chart with series of different magnitudes. Hide the large-value series and verify the Y-axis rescales to fit the remaining visible series.
2. **Edge cases**:
   - Hide all series for one unit → scale should fall back to full range
   - Hide all series entirely → scales should not collapse to zero
   - Toggle series back to visible → scale should re-expand
   - Ctrl+Click individual toggles → scale adapts per toggle
   - Click to isolate → scale adapts to isolated series
3. **Run lint/type-check**: `cd analytics-web-app && yarn lint && yarn type-check`
