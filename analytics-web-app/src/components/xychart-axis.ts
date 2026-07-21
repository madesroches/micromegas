/**
 * Pure uPlot X-axis config builder for XYChart.
 *
 * Extracted from XYChart's chart-construction effect (#1089) so the axis
 * formatting (categorical label lookup, numeric abbreviation) is isolated and
 * unit-testable. `import type` keeps this free of a runtime cycle with XYChart.
 */
import type uPlot from 'uplot'
import type { XAxisMode } from './XYChart'
import { formatCurrencyValue } from '@/lib/units'

export function buildXAxisConfig(xAxisMode: XAxisMode, xLabels?: string[]): uPlot.Axis {
  const xAxisConfig: uPlot.Axis = {
    stroke: '#6a6a7a',
    grid: { stroke: '#2a2a35', width: 1 },
    ticks: { stroke: '#2a2a35', width: 1 },
    font: '11px -apple-system, BlinkMacSystemFont, sans-serif',
    size: 65,
  }

  if (xAxisMode === 'categorical' && xLabels) {
    xAxisConfig.incrs = [1]
    xAxisConfig.space = 60
    xAxisConfig.values = (_u: uPlot, vals: number[]) => {
      return vals.map((v) => {
        const idx = Math.round(v)
        if (idx >= 0 && idx < xLabels.length) return xLabels[idx]
        return ''
      })
    }
  } else if (xAxisMode === 'numeric') {
    xAxisConfig.space = 60
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

  return xAxisConfig
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
  if (dv === 0) return '0 ' + displayUnit
  const absV = Math.abs(dv)
  if (absV >= 100) return Math.round(dv) + ' ' + displayUnit
  if (absV >= 10) return dv.toFixed(1) + ' ' + displayUnit
  if (absV >= 1) return dv.toFixed(2) + ' ' + displayUnit
  return dv.toPrecision(2) + ' ' + displayUnit
}
