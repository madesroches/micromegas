/**
 * Pure uPlot X-axis config builder for XYChart.
 *
 * Extracted from XYChart's chart-construction effect (#1089) so the axis
 * formatting (categorical label lookup, numeric abbreviation) is isolated and
 * unit-testable. `import type` keeps this free of a runtime cycle with XYChart.
 */
import type uPlot from 'uplot'
import type { XAxisMode } from './XYChart'

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
