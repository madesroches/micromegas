/**
 * Pure uPlot X-axis config builder for XYChart.
 *
 * Extracted from XYChart's chart-construction effect (#1089) so the axis
 * formatting (categorical label lookup, numeric abbreviation) is isolated and
 * unit-testable. `import type` keeps this free of a runtime cycle with XYChart.
 */
import type uPlot from 'uplot'
import type { XAxisMode } from './XYChart'
import { formatCurrencyValue, unitSuffix } from '@/lib/units'

// Flat (non-rotated) axis height/space, unchanged from before adaptive rotation existed.
export const BASE_SIZE = 65
export const BASE_MIN_SPACE_PX = 60

// Overlap heuristic: approximate label pixel width from character count rather than
// ctx.measureText() (no canvas context in this pure, unit-tested module — see plan Trade-offs).
export const AVG_CHAR_WIDTH_PX = 6 // approx. glyph advance for the 11px sans-serif axis font

// Small buffer so labels rotate slightly before they'd visually touch.
export const TICK_LABEL_PADDING_PX = 8

// Fixed rotation angle for overlapping categorical labels.
export const ROTATE_DEG = -45

// Approximate single-line text height for the 11px axis font.
export const LABEL_LINE_HEIGHT_PX = 14

// Tick length + label gap, matching the existing (non-rotated) visual spacing.
export const AXIS_CHROME_PX = 20

// Rotated size/padding cap: min(MAX_ROTATED_SIZE, round(dimension * ROTATED_SIZE_FRACTION)).
export const MAX_ROTATED_SIZE = 160
export const ROTATED_SIZE_FRACTION = 0.4

// Mirrors uPlot's own autoPadSide right-edge cushion (round(yAxisOpts.size / 2), default
// yAxisOpts.size = 50) so non-rotated categorical charts keep the same right-edge clearance
// a `padding` function can't fall back to `null` for once installed.
export const DEFAULT_RIGHT_CUSHION_PX = 25

// Mirrors XYChart.tsx's right-side y-axis `size: 90`, the clearance a rotated label can
// safely cross into when a right y-axis is present.
export const RIGHT_AXIS_SIZE_PX = 90

// Once labels are rotated they no longer need to fit horizontally within a tick slot;
// adjacent rotated baselines only need to clear LABEL_LINE_HEIGHT_PX / sin(45deg) apart.
export const ROTATED_MIN_SPACE_PX = 20

/** Estimate a label's rendered pixel width from its character count (see module-level comment). */
export function estimateLabelWidth(label: string): number {
  return label.length * AVG_CHAR_WIDTH_PX
}

export function buildXAxisConfig(
  xAxisMode: XAxisMode,
  xLabels?: string[]
): { axis: uPlot.Axis; rightPadding: uPlot.PaddingSide } {
  const xAxisConfig: uPlot.Axis = {
    stroke: '#6a6a7a',
    grid: { stroke: '#2a2a35', width: 1 },
    ticks: { stroke: '#2a2a35', width: 1 },
    font: '11px -apple-system, BlinkMacSystemFont, sans-serif',
    size: BASE_SIZE,
  }
  // Degrades to uPlot's own autoPadSide cushion on the right edge for time/numeric mode
  // (a literal 0 would override that cushion away instead).
  let rightPadding: uPlot.PaddingSide = null

  if (xAxisMode === 'categorical' && xLabels) {
    xAxisConfig.incrs = [1]
    xAxisConfig.values = (_u: uPlot, vals: number[]) => {
      return vals.map((v) => {
        const idx = Math.round(v)
        if (idx >= 0 && idx < xLabels.length) return xLabels[idx]
        return ''
      })
    }

    // Shared mutable state: `rotate()` is the sole writer, the other three callbacks below
    // are pure readers. Safe because uPlot always calls rotate() immediately before size()
    // for the same axis in the same cycle, and paddingCalc() right after axesCalc() in that
    // same cycle — see the plan's Design section for the full trace.
    let rotated = false
    let maxWidth = 0

    xAxisConfig.rotate = (_u, values, _axisIdx, foundSpace) => {
      maxWidth = Math.max(0, ...values.map((v) => estimateLabelWidth(String(v))))
      rotated = maxWidth + TICK_LABEL_PADDING_PX > foundSpace
      return rotated ? ROTATE_DEG : 0
    }

    xAxisConfig.size = (self) => {
      if (!rotated) return BASE_SIZE
      const angleRad = (Math.abs(ROTATE_DEG) * Math.PI) / 180
      const rotatedExtent = maxWidth * Math.sin(angleRad) + LABEL_LINE_HEIGHT_PX * Math.cos(angleRad)
      const cap = Math.min(MAX_ROTATED_SIZE, Math.round(self.height * ROTATED_SIZE_FRACTION))
      return Math.min(cap, Math.ceil(rotatedExtent) + AXIS_CHROME_PX)
    }

    // incrs = [1] pins tick density, so this floor is purely a blank/don't-blank gate,
    // never a tick-density control (see plan's `space` subsection).
    xAxisConfig.space = () => (rotated ? ROTATED_MIN_SPACE_PX : BASE_MIN_SPACE_PX)

    rightPadding = (self, _side, sidesWithAxes) => {
      // Not rotated: reproduce uPlot's own autoPadSide result for the right edge, since a
      // function's numeric return can never fall back to it. Mirrors autoPadSide's side-1
      // branch exactly, including its (hasTopAxis || hasBtmAxis) guard, not just hasRgtAxis.
      if (!rotated) return (sidesWithAxes[0] || sidesWithAxes[2]) && !sidesWithAxes[1] ? DEFAULT_RIGHT_CUSHION_PX : 0
      const angleRad = (Math.abs(ROTATE_DEG) * Math.PI) / 180
      const horizontalExtent = maxWidth * Math.cos(angleRad) + LABEL_LINE_HEIGHT_PX * Math.sin(angleRad)
      const cap = Math.min(MAX_ROTATED_SIZE, Math.round(self.width * ROTATED_SIZE_FRACTION))
      const capped = Math.min(cap, Math.ceil(horizontalExtent))
      // A right y-axis already reserves clearance rotated labels can safely cross into, so
      // subtract that band instead of reserving the full projection on top of it.
      return Math.max(0, capped - (sidesWithAxes[1] ? RIGHT_AXIS_SIZE_PX : 0))
    }
  } else if (xAxisMode === 'numeric') {
    xAxisConfig.space = BASE_MIN_SPACE_PX
    xAxisConfig.values = (_u: uPlot, vals: number[]) => {
      return vals.map((v) => {
        if (v === 0) return '0'
        const absV = Math.abs(v)
        if (absV >= 1000) return v.toLocaleString()
        if (absV >= 1) return v.toFixed(1)
        return v.toPrecision(2)
      })
    }
  }

  return { axis: xAxisConfig, rightPadding }
}

export function buildXScale(xAxisMode: XAxisMode): uPlot.Scale {
  const scale: uPlot.Scale = { time: xAxisMode === 'time' }
  if (xAxisMode === 'categorical') {
    // Pad by half a slot so end bars aren't clipped and end labels stay
    // centered under their bars. A slot is 1 index wide; bars span 0.8 of it.
    scale.range = (_u, dataMin, dataMax) => [dataMin - 0.5, dataMax + 0.5]
  }
  return scale
}

/**
 * Pure Y-axis tick formatter shared by XYChart's multi-series (per-unit-scale)
 * and single-series numeric axes. Extracted alongside `buildXAxisConfig` so
 * the branching (currency vs. plain-number + unit-suffix) is unit-testable.
 *
 * `rawValue` is the value as passed into the axis `values` callback;
 * `axisConversionFactor` is applied on top of it (pass `1` when the caller
 * has already pre-scaled the data, as the single-series path does).
 * `currencyCode` is the raw (un-normalized) currency unit string when the
 * axis is a currency scale, or `null` otherwise.
 */
export function formatYAxisTick(
  rawValue: number,
  axisConversionFactor: number,
  displayUnit: string,
  currencyCode: string | null,
): string {
  const dv = rawValue * axisConversionFactor
  if (currencyCode) return formatCurrencyValue(dv, currencyCode)
  const suffix = unitSuffix(displayUnit)
  if (dv === 0) return '0' + suffix
  const absV = Math.abs(dv)
  if (absV >= 100) return Math.round(dv) + suffix
  if (absV >= 10) return dv.toFixed(1) + suffix
  if (absV >= 1) return dv.toFixed(2) + suffix
  return dv.toPrecision(2) + suffix
}
