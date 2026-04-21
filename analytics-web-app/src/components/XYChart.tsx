import { useEffect, useRef, useState, useCallback, useMemo } from 'react'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'
import {
  isTimeUnit,
  getAdaptiveTimeUnit,
  formatAdaptiveTime,
  formatTimeValue,
  type AdaptiveTimeUnit,
} from '@/lib/time-units'
import { normalizeUnit, isSizeUnit, getAdaptiveSizeUnit, isBitUnit, getAdaptiveBitUnit } from '@/lib/units'
import type { ChartSeriesData } from '@/lib/arrow-utils'

import { SERIES_COLORS } from './chart-constants'

export interface ChartAxisBounds {
  left: number // Left padding (Y-axis width)
  width: number // Plot area width
}

export type ScaleMode = 'p99' | 'max'

export type ChartType = 'line' | 'bar'

export type XAxisMode = 'time' | 'numeric' | 'categorical'

function escapeHtml(str: string): string {
  return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
}

function hexToRgba(hex: string, alpha: number): string {
  const r = parseInt(hex.slice(1, 3), 16)
  const g = parseInt(hex.slice(3, 5), 16)
  const b = parseInt(hex.slice(5, 7), 16)
  return `rgba(${r}, ${g}, ${b}, ${alpha})`
}

interface XYChartProps {
  data?: { x: number; y: number }[] // categorical: x is index into xLabels
  xAxisMode: XAxisMode // required, determined by extractChartData
  xLabels?: string[] // for categorical mode - the actual string labels
  xColumnName?: string
  yColumnName?: string
  title?: string
  unit?: string
  // Multi-series
  series?: ChartSeriesData[]
  scaleMode?: ScaleMode
  onScaleModeChange?: (mode: ScaleMode) => void
  chartType?: ChartType
  onChartTypeChange?: (type: ChartType) => void
  onTimeRangeSelect?: (from: Date, to: Date) => void
  onWidthChange?: (width: number) => void
  onAxisBoundsChange?: (bounds: ChartAxisBounds) => void
}

function formatValue(
  value: number,
  rawUnit: string,
  abbreviated = false,
  adaptiveTimeUnit?: AdaptiveTimeUnit
): string {
  const unit = normalizeUnit(rawUnit)

  // Use adaptive formatting for time units
  if (adaptiveTimeUnit && isTimeUnit(unit)) {
    return formatAdaptiveTime(value, adaptiveTimeUnit, abbreviated)
  }

  // Size units - use adaptive formatting
  if (isSizeUnit(unit)) {
    const adaptive = getAdaptiveSizeUnit(value, unit)
    const displayValue = value * adaptive.conversionFactor
    const decimals = adaptive.unit === 'bytes' ? 0 : 1
    return displayValue.toFixed(decimals) + ' ' + adaptive.abbrev
  }

  // Rate units - bytes per second (uses same adaptive logic)
  if (unit === 'bytes/s') {
    const adaptive = getAdaptiveSizeUnit(value, 'bytes')
    const displayValue = value * adaptive.conversionFactor
    const decimals = adaptive.unit === 'bytes' ? 0 : 1
    return displayValue.toFixed(decimals) + ' ' + adaptive.abbrev + '/s'
  }

  // Bit units - networking, decimal scaling
  if (isBitUnit(unit)) {
    const adaptive = getAdaptiveBitUnit(value, unit)
    const displayValue = value * adaptive.conversionFactor
    const decimals = adaptive.unit === 'bits' ? 0 : 1
    return displayValue.toFixed(decimals) + ' ' + adaptive.abbrev
  }

  // Rate units - bits per second
  if (unit === 'bits/s') {
    const adaptive = getAdaptiveBitUnit(value, 'bits')
    const displayValue = value * adaptive.conversionFactor
    const decimals = adaptive.unit === 'bits' ? 0 : 1
    return displayValue.toFixed(decimals) + ' ' + adaptive.abbrev + '/s'
  }

  // Other units
  if (unit === 'percent') return value.toFixed(1) + '%'
  if (unit === 'degrees') return value.toFixed(1) + '°'
  if (unit === 'boolean') return value !== 0 ? 'true' : 'false'

  // Default: show number, append unit if provided
  return rawUnit ? `${value.toLocaleString()} ${rawUnit}` : value.toLocaleString()
}

// Format a stat value - for time units, each value picks its own best unit
function formatStatValue(value: number, unit: string): string {
  if (isTimeUnit(unit)) {
    return formatTimeValue(value, unit, false)
  }
  return formatValue(value, unit, false)
}

// Format X value based on axis mode
function formatXValue(value: number, mode: XAxisMode, xLabels?: string[]): string {
  switch (mode) {
    case 'time': {
      const date = new Date(value * 1000)
      const timeStr =
        date.toLocaleTimeString('en-US', {
          hour: '2-digit',
          minute: '2-digit',
          second: '2-digit',
          hour12: false,
        }) +
        '.' +
        String(date.getMilliseconds()).padStart(3, '0')
      return timeStr
    }
    case 'numeric':
      if (Math.abs(value) >= 1000) return value.toLocaleString()
      if (Math.abs(value) >= 1) return value.toFixed(2)
      return value.toPrecision(3)
    case 'categorical':
      if (xLabels) {
        const idx = Math.round(value)
        if (idx >= 0 && idx < xLabels.length) {
          return xLabels[idx]
        }
      }
      return String(Math.round(value))
  }
}

// Compute stats for a series
function computeStats(values: number[]) {
  if (values.length === 0) return { min: 0, max: 0, avg: 0, p99: 0 }
  const sorted = [...values].sort((a, b) => a - b)
  const p99Index = Math.floor(sorted.length * 0.99)
  return {
    min: sorted[0],
    max: sorted[sorted.length - 1],
    avg: values.reduce((sum, v) => sum + v, 0) / values.length,
    p99: sorted[Math.min(p99Index, sorted.length - 1)],
  }
}

export function XYChart({
  data,
  xAxisMode,
  xLabels,
  xColumnName,
  yColumnName,
  title = '',
  unit = '',
  series: seriesProp,
  scaleMode: scaleModeFromProps,
  onScaleModeChange,
  chartType: chartTypeFromProps,
  onChartTypeChange,
  onTimeRangeSelect,
  onWidthChange,
  onAxisBoundsChange,
}: XYChartProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const chartRef = useRef<uPlot | null>(null)
  const [dimensions, setDimensions] = useState({ width: 800, height: 300 })
  const [internalScaleMode, setInternalScaleMode] = useState<ScaleMode>('p99')
  const [internalChartType, setInternalChartType] = useState<ChartType>('line')
  const [seriesVisibility, setSeriesVisibility] = useState<boolean[] | null>(null)
  const [isolatedSeries, setIsolatedSeries] = useState<number | null>(null)

  // Use refs for callbacks to avoid chart recreation when callbacks change
  const onTimeRangeSelectRef = useRef(onTimeRangeSelect)
  onTimeRangeSelectRef.current = onTimeRangeSelect
  const onAxisBoundsChangeRef = useRef(onAxisBoundsChange)
  onAxisBoundsChangeRef.current = onAxisBoundsChange

  // Use prop if provided, otherwise use internal state
  const scaleMode = scaleModeFromProps ?? internalScaleMode
  const setScaleMode = onScaleModeChange ?? setInternalScaleMode
  const chartType = chartTypeFromProps ?? internalChartType
  const setChartType = onChartTypeChange ?? setInternalChartType

  // Determine if multi-series mode
  const isMultiSeries = !!seriesProp && seriesProp.length > 0
  const effectiveSeriesCount = isMultiSeries ? seriesProp!.length : 1

  // Normalize single-series data into multi-series format for unified processing
  const normalizedSeries: ChartSeriesData[] = useMemo(() => {
    if (isMultiSeries) return seriesProp!
    if (!data || data.length === 0) return []
    return [{
      label: title || yColumnName || 'Value',
      unit: unit,
      data: data,
    }]
  }, [isMultiSeries, seriesProp, data, title, yColumnName, unit])

  // Calculate per-series stats
  const allSeriesStats = useMemo(() => {
    return normalizedSeries.map(s => computeStats(s.data.map(d => d.y)))
  }, [normalizedSeries])

  // Stats for the first series (used for single-series header display)
  const stats = allSeriesStats[0] ?? { min: 0, max: 0, avg: 0, p99: 0 }

  // Build per-unit scale info (group series by unit)
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
    // Convert to ordered array: first unit = left axis, second = right, etc.
    const entries = [...unitMap.entries()]
    return entries.map(([unitName, info], idx) => ({
      unitName,
      scaleName: unitName || 'y',
      side: idx === 0 ? 1 : idx === 1 ? 3 : idx % 2 === 0 ? 1 : 3, // 1=left, 3=right, alternate
      ...info,
    }))
  }, [normalizedSeries, allSeriesStats, seriesVisibility])

  // For single-series, use first series unit for adaptive formatting
  const primaryUnit = normalizedSeries[0]?.unit || unit || ''

  // Calculate adaptive time unit based on p99 value
  const adaptiveTimeUnit = useMemo(() => {
    if (!isTimeUnit(primaryUnit) || stats.p99 === 0) return undefined
    return getAdaptiveTimeUnit(stats.p99, primaryUnit)
  }, [primaryUnit, stats.p99])

  // Calculate adaptive size unit based on p99 value
  const adaptiveSizeUnit = useMemo(() => {
    if (!isSizeUnit(primaryUnit) || stats.p99 === 0) return undefined
    return getAdaptiveSizeUnit(stats.p99, primaryUnit)
  }, [primaryUnit, stats.p99])

  // Calculate adaptive bit unit based on p99 value
  const adaptiveBitUnit = useMemo(() => {
    if (!isBitUnit(primaryUnit) || stats.p99 === 0) return undefined
    return getAdaptiveBitUnit(stats.p99, primaryUnit)
  }, [primaryUnit, stats.p99])

  // Display unit for the header (adaptive abbreviation for time/size/bits, original for others).
  // Using `.abbrev` across all three keeps the header consistent with the y-axis label below.
  const displayUnit = adaptiveTimeUnit?.abbrev ?? adaptiveSizeUnit?.abbrev ?? adaptiveBitUnit?.abbrev ?? primaryUnit

  // Use ref for onWidthChange to avoid effect re-runs when callback identity changes
  const onWidthChangeRef = useRef(onWidthChange)
  onWidthChangeRef.current = onWidthChange

  // Track last reported width to avoid duplicate callbacks
  const lastReportedWidthRef = useRef<number | null>(null)

  // Measure container and update dimensions
  const measureContainer = useCallback(() => {
    if (containerRef.current) {
      const rect = containerRef.current.getBoundingClientRect()
      const newWidth = Math.round(Math.max(400, rect.width - 32))
      const newHeight = Math.round(Math.max(250, rect.height - 32))

      setDimensions((prev) => {
        if (prev.width === newWidth && prev.height === newHeight) return prev
        return { width: newWidth, height: newHeight }
      })

      return newWidth
    }
    return null
  }, [])

  // Handle initial mount measurement
  useEffect(() => {
    const width = measureContainer()
    if (width !== null && lastReportedWidthRef.current !== width) {
      lastReportedWidthRef.current = width
      onWidthChangeRef.current?.(width)
    }

    const resizeObserver = new ResizeObserver(() => { measureContainer() })
    if (containerRef.current) {
      resizeObserver.observe(containerRef.current)
    }

    return () => resizeObserver.disconnect()
  }, [measureContainer])

  // Handle window resize
  useEffect(() => {
    const handleWindowResize = () => {
      const width = measureContainer()
      if (width !== null && lastReportedWidthRef.current !== width) {
        lastReportedWidthRef.current = width
        onWidthChangeRef.current?.(width)
      }
    }

    window.addEventListener('resize', handleWindowResize)
    return () => window.removeEventListener('resize', handleWindowResize)
  }, [measureContainer])

  // Multi-series tooltip plugin
  const createMultiSeriesTooltipPlugin = useCallback(
    (seriesInfo: { label: string; unit: string; color: string }[], mode: XAxisMode, labels?: string[]): uPlot.Plugin => {
      let tooltip: HTMLDivElement
      let overEl: HTMLElement

      return {
        hooks: {
          init: (u: uPlot) => {
            overEl = u.over
            tooltip = document.createElement('div')
            tooltip.style.cssText = `
              position: fixed;
              background: var(--app-bg);
              border: 1px solid var(--border-color);
              border-radius: 6px;
              padding: 10px 14px;
              font-size: 12px;
              pointer-events: none;
              z-index: 100;
              box-shadow: 0 4px 12px rgba(0,0,0,0.5);
              display: none;
            `
            document.body.appendChild(tooltip)
          },
          setCursor: (u: uPlot) => {
            const { idx, left, top } = u.cursor
            if (idx == null || left == null || top == null || left < 0 || top < 0) {
              tooltip.style.display = 'none'
              return
            }

            const xVal = u.data[0][idx]
            if (xVal == null) {
              tooltip.style.display = 'none'
              return
            }

            let html = `<div style="color: var(--text-muted); margin-bottom: 6px; font-family: monospace; font-size: 11px;">${escapeHtml(formatXValue(xVal, mode, labels))}</div>`

            let hasValues = false
            for (let i = 0; i < seriesInfo.length; i++) {
              const value = u.data[i + 1]?.[idx]
              const info = seriesInfo[i]
              const safeLabel = escapeHtml(info.label)
              if (value == null) {
                html += `<div style="display: flex; align-items: center; gap: 8px; padding: 2px 0;">
                  <div style="width: 8px; height: 8px; border-radius: 50%; background: ${info.color};"></div>
                  <span style="color: #6a6a7a; min-width: 90px;">${safeLabel}</span>
                  <span style="color: #6a6a7a;">&mdash;</span>
                </div>`
              } else {
                hasValues = true
                const formatted = isTimeUnit(info.unit)
                  ? formatTimeValue(value, info.unit)
                  : formatStatValue(value, info.unit)
                html += `<div style="display: flex; align-items: center; gap: 8px; padding: 2px 0;">
                  <div style="width: 8px; height: 8px; border-radius: 50%; background: ${info.color};"></div>
                  <span style="color: #b0b0c0; min-width: 90px;">${safeLabel}</span>
                  <span style="color: #e0e0e8; font-weight: 600; font-size: 13px;">${escapeHtml(formatted)}</span>
                </div>`
              }
            }

            if (!hasValues) {
              tooltip.style.display = 'none'
              return
            }

            tooltip.innerHTML = html

            // Position tooltip using fixed coordinates (immune to overflow clipping)
            const rect = overEl.getBoundingClientRect()
            const tooltipHeight = 30 + seriesInfo.length * 26
            const flipThreshold = tooltipHeight + 10
            const posTop = top < flipThreshold ? rect.top + top + 20 : rect.top + top - tooltipHeight

            tooltip.style.left = rect.left + left + 12 + 'px'
            tooltip.style.top = posTop + 'px'
            tooltip.style.display = 'block'
          },
          destroy: () => {
            if (tooltip && tooltip.parentNode) {
              tooltip.parentNode.removeChild(tooltip)
            }
          },
        },
      }
    },
    []
  )

  // Single-series tooltip plugin (preserved for backward compat)
  const createTooltipPlugin = useCallback(
    (originalUnit: string, conversionFactor: number, mode: XAxisMode, labels?: string[]): uPlot.Plugin => {
      let tooltip: HTMLDivElement
      let tooltipX: HTMLDivElement
      let tooltipValue: HTMLDivElement
      let overEl: HTMLElement

      return {
        hooks: {
          init: (u: uPlot) => {
            overEl = u.over
            tooltip = document.createElement('div')
            tooltip.style.cssText = `
            position: fixed;
            background: var(--app-bg);
            border: 1px solid var(--border-color);
            border-radius: 6px;
            padding: 8px 12px;
            font-size: 12px;
            pointer-events: none;
            z-index: 100;
            box-shadow: 0 4px 12px rgba(0,0,0,0.4);
            display: none;
          `
            tooltip.innerHTML = `
            <div style="color: var(--text-muted); margin-bottom: 4px; font-family: monospace;"></div>
            <div style="color: var(--chart-line); font-weight: 600; font-size: 14px;"></div>
          `
            document.body.appendChild(tooltip)
            tooltipX = tooltip.children[0] as HTMLDivElement
            tooltipValue = tooltip.children[1] as HTMLDivElement
          },
          setCursor: (u: uPlot) => {
            const { idx, left, top } = u.cursor
            if (idx == null || left == null || top == null || left < 0 || top < 0) {
              tooltip.style.display = 'none'
              return
            }

            const xVal = u.data[0][idx]
            const value = u.data[1][idx]

            if (xVal == null || value == null) {
              tooltip.style.display = 'none'
              return
            }

            tooltipX.textContent = formatXValue(xVal, mode, labels)

            // Convert back to original unit and pick best unit for display
            const originalValue = value / conversionFactor
            if (isTimeUnit(originalUnit)) {
              tooltipValue.textContent = formatTimeValue(originalValue, originalUnit)
            } else {
              tooltipValue.textContent = formatStatValue(originalValue, originalUnit)
            }

            // Position tooltip using fixed coordinates (immune to overflow clipping)
            const rect = overEl.getBoundingClientRect()
            const tooltipHeight = 60
            const flipThreshold = tooltipHeight + 10
            const posTop = top < flipThreshold ? rect.top + top + 20 : rect.top + top - tooltipHeight

            tooltip.style.left = rect.left + left + 10 + 'px'
            tooltip.style.top = posTop + 'px'
            tooltip.style.display = 'block'
          },
          destroy: () => {
            if (tooltip && tooltip.parentNode) {
              tooltip.parentNode.removeChild(tooltip)
            }
          },
        },
      }
    },
    []
  )

  // Reset series visibility when series count changes
  useEffect(() => {
    setSeriesVisibility(null)
    setIsolatedSeries(null)
  }, [effectiveSeriesCount])

  // Create/update chart
  useEffect(() => {
    if (!containerRef.current) return
    const hasData = isMultiSeries
      ? normalizedSeries.some(s => s.data.length > 0)
      : (data && data.length > 0)
    if (!hasData) return

    // Destroy previous chart
    if (chartRef.current) {
      chartRef.current.destroy()
      chartRef.current = null
    }

    // Build X axis configuration based on mode
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

    if (isMultiSeries && normalizedSeries.length > 1) {
      // ===================== MULTI-SERIES PATH =====================

      // Build union of all X values
      const xSet = new Set<number>()
      for (const s of normalizedSeries) {
        for (const d of s.data) {
          const xVal = xAxisMode === 'time' ? d.x / 1000 : d.x
          xSet.add(xVal)
        }
      }
      const unionX = [...xSet].sort((a, b) => a - b)

      // Build data arrays: [unionX, series1Values, series2Values, ...]
      // Each series maps onto the union X array, with null for missing points
      const uPlotData: (number | null)[][] = [unionX as (number | null)[]]
      const xIndex = new Map<number, number>()
      unionX.forEach((v, i) => xIndex.set(v, i))

      for (const s of normalizedSeries) {
        const yArr: (number | null)[] = new Array(unionX.length).fill(null)
        for (const d of s.data) {
          const xVal = xAxisMode === 'time' ? d.x / 1000 : d.x
          const idx = xIndex.get(xVal)
          if (idx != null) yArr[idx] = d.y
        }
        uPlotData.push(yArr)
      }

      // Build scales, axes, and series configs
      const scales: uPlot.Scales = {
        x: { time: xAxisMode === 'time' },
      }
      const axes: uPlot.Axis[] = [xAxisConfig]
      const uPlotSeries: uPlot.Series[] = [{}]

      // Compute adaptive unit info per scale for auto-scaling axis labels
      const unitAdaptiveMap = new Map<string, { conversionFactor: number; abbrev: string }>()
      for (const scaleInfo of unitScaleInfo) {
        const normalizedUnit = normalizeUnit(scaleInfo.unitName)
        if (isTimeUnit(normalizedUnit) && scaleInfo.p99 > 0) {
          const adaptive = getAdaptiveTimeUnit(scaleInfo.p99, normalizedUnit)
          unitAdaptiveMap.set(scaleInfo.unitName, { conversionFactor: adaptive.conversionFactor, abbrev: adaptive.abbrev })
        } else if (isSizeUnit(normalizedUnit) && scaleInfo.p99 > 0) {
          const adaptive = getAdaptiveSizeUnit(scaleInfo.p99, normalizedUnit)
          unitAdaptiveMap.set(scaleInfo.unitName, { conversionFactor: adaptive.conversionFactor, abbrev: adaptive.abbrev })
        } else if (isBitUnit(normalizedUnit) && scaleInfo.p99 > 0) {
          const adaptive = getAdaptiveBitUnit(scaleInfo.p99, normalizedUnit)
          unitAdaptiveMap.set(scaleInfo.unitName, { conversionFactor: adaptive.conversionFactor, abbrev: adaptive.abbrev })
        }
      }

      // Build per-unit axes
      for (const scaleInfo of unitScaleInfo) {
        const scaleName = scaleInfo.scaleName
        const scaleP99 = scaleInfo.p99
        const scaleMax = scaleInfo.max

        scales[scaleName] = {
          range: (_u: uPlot, dataMin: number, _dataMax: number) => {
            const minVal = Math.min(0, dataMin)
            const scaleValue = scaleMode === 'p99' ? scaleP99 : scaleMax
            const maxVal = scaleValue * 1.05
            return [minVal, Math.max(maxVal, 0.001)]
          },
        }

        const adaptiveInfo = unitAdaptiveMap.get(scaleInfo.unitName)
        const yAxisUnit = adaptiveInfo?.abbrev ?? (scaleInfo.unitName === 'percent' ? '%' : scaleInfo.unitName)
        const axisCf = adaptiveInfo?.conversionFactor ?? 1
        axes.push({
          show: scaleInfo.hasVisible,
          scale: scaleName,
          side: scaleInfo.side as 1 | 3,
          stroke: '#6a6a7a',
          grid: scaleInfo.side === 1 ? { stroke: '#2a2a35', width: 1 } : { show: false },
          ticks: { stroke: '#2a2a35', width: 1 },
          font: '11px -apple-system, BlinkMacSystemFont, sans-serif',
          size: 90,
          values: (_u: uPlot, vals: number[]) => {
            return vals.map((v) => {
              const dv = v * axisCf
              if (v === 0) return '0 ' + yAxisUnit
              const absV = Math.abs(dv)
              if (absV >= 100) return Math.round(dv) + ' ' + yAxisUnit
              if (absV >= 10) return dv.toFixed(1) + ' ' + yAxisUnit
              if (absV >= 1) return dv.toFixed(2) + ' ' + yAxisUnit
              return dv.toPrecision(2) + ' ' + yAxisUnit
            })
          },
        })
      }

      // Build uPlot series configs
      const seriesInfoForTooltip: { label: string; unit: string; color: string }[] = []
      for (let i = 0; i < normalizedSeries.length; i++) {
        const s = normalizedSeries[i]
        const color = SERIES_COLORS[i % SERIES_COLORS.length]
        const scaleName = s.unit || 'y'

        uPlotSeries.push({
          label: s.label,
          scale: scaleName,
          stroke: color,
          width: chartType === 'bar' ? 1 : 2,
          fill: chartType === 'bar' ? hexToRgba(color, 0.6) : hexToRgba(color, 0.1),
          paths: chartType === 'bar'
            ? uPlot.paths.bars!({ size: [0.8 / normalizedSeries.length], gap: 1, align: i as never })
            : undefined,
          points: { show: chartType !== 'bar' },
          show: seriesVisibility ? seriesVisibility[i] : true,
        })

        seriesInfoForTooltip.push({ label: s.label, unit: s.unit, color })
      }

      const opts: uPlot.Options = {
        width: dimensions.width,
        height: dimensions.height,
        plugins: [createMultiSeriesTooltipPlugin(seriesInfoForTooltip, xAxisMode, xLabels)],
        tzDate: xAxisMode === 'time' ? (ts: number) => new Date(ts * 1000) : undefined,
        scales,
        axes,
        series: uPlotSeries,
        cursor: {
          show: true,
          x: true,
          y: true,
          drag: {
            x: xAxisMode === 'time',
            y: false,
            setScale: false,
          },
        },
        hooks: {
          ready: [
            (u: uPlot) => {
              onAxisBoundsChangeRef.current?.({
                left: u.bbox.left / devicePixelRatio,
                width: u.bbox.width / devicePixelRatio,
              })
            },
          ],
          setSize: [
            (u: uPlot) => {
              onAxisBoundsChangeRef.current?.({
                left: u.bbox.left / devicePixelRatio,
                width: u.bbox.width / devicePixelRatio,
              })
            },
          ],
          setSelect: [
            (u: uPlot) => {
              if (xAxisMode !== 'time') return
              const { left, width } = u.select
              if (width > 0 && onTimeRangeSelectRef.current) {
                const fromTime = u.posToVal(left, 'x')
                const toTime = u.posToVal(left + width, 'x')
                const fromDate = new Date(fromTime * 1000)
                const toDate = new Date(toTime * 1000)
                u.setSelect({ left: 0, width: 0, top: 0, height: 0 }, false)
                onTimeRangeSelectRef.current(fromDate, toDate)
              }
            },
          ],
        },
        legend: { show: false },
      }

      const chartContainer = containerRef.current.querySelector('.chart-inner') as HTMLElement
      if (chartContainer) {
        chartRef.current = new uPlot(opts, uPlotData as uPlot.AlignedData, chartContainer)
      }
    } else {
      // ===================== SINGLE-SERIES PATH =====================
      const singleData = normalizedSeries[0]?.data ?? data ?? []
      if (singleData.length === 0) return

      const conversionFactor = adaptiveTimeUnit?.conversionFactor ?? adaptiveSizeUnit?.conversionFactor ?? adaptiveBitUnit?.conversionFactor ?? 1

      const xValues = xAxisMode === 'time'
        ? singleData.map((d) => d.x / 1000)
        : singleData.map((d) => d.x)
      const yValues = singleData.map((d) => d.y * conversionFactor)

      const displayP99 = stats.p99 * conversionFactor
      const displayMax = stats.max * conversionFactor

      const yAxisUnit = adaptiveTimeUnit?.abbrev ?? adaptiveSizeUnit?.abbrev ?? adaptiveBitUnit?.abbrev ?? (primaryUnit === 'percent' ? '%' : primaryUnit)

      const opts: uPlot.Options = {
        width: dimensions.width,
        height: dimensions.height,
        plugins: [createTooltipPlugin(primaryUnit, conversionFactor, xAxisMode, xLabels)],
        tzDate: xAxisMode === 'time' ? (ts: number) => new Date(ts * 1000) : undefined,
        scales: {
          x: { time: xAxisMode === 'time' },
          y: {
            range: (_u: uPlot, dataMin: number, _dataMax: number) => {
              const minVal = Math.min(0, dataMin)
              const scaleValue = scaleMode === 'p99' ? displayP99 : displayMax
              const maxVal = scaleValue * 1.05
              return [minVal, maxVal]
            },
          },
        },
        axes: [
          xAxisConfig,
          {
            stroke: '#6a6a7a',
            grid: { stroke: '#2a2a35', width: 1 },
            ticks: { stroke: '#2a2a35', width: 1 },
            font: '11px -apple-system, BlinkMacSystemFont, sans-serif',
            size: 90,
            values: (_u: uPlot, vals: number[]) => {
              return vals.map((v) => {
                if (v === 0) return '0 ' + yAxisUnit
                const absV = Math.abs(v)
                if (absV >= 100) return Math.round(v) + ' ' + yAxisUnit
                if (absV >= 10) return v.toFixed(1) + ' ' + yAxisUnit
                if (absV >= 1) return v.toFixed(2) + ' ' + yAxisUnit
                return v.toPrecision(2) + ' ' + yAxisUnit
              })
            },
          },
        ],
        series: [
          {},
          {
            label: title || yColumnName || 'Value',
            stroke: '#bf360c',
            width: chartType === 'bar' ? 1 : 2,
            fill: chartType === 'bar' ? 'rgba(191, 54, 12, 0.6)' : 'rgba(191, 54, 12, 0.1)',
            paths: chartType === 'bar' ? uPlot.paths.bars!({ size: [0.8], gap: 1 }) : undefined,
            points: { show: chartType !== 'bar' },
          },
        ],
        cursor: {
          show: true,
          x: true,
          y: true,
          drag: {
            x: xAxisMode === 'time',
            y: false,
            setScale: false,
          },
        },
        hooks: {
          ready: [
            (u: uPlot) => {
              onAxisBoundsChangeRef.current?.({
                left: u.bbox.left / devicePixelRatio,
                width: u.bbox.width / devicePixelRatio,
              })
            },
          ],
          setSize: [
            (u: uPlot) => {
              onAxisBoundsChangeRef.current?.({
                left: u.bbox.left / devicePixelRatio,
                width: u.bbox.width / devicePixelRatio,
              })
            },
          ],
          setSelect: [
            (u: uPlot) => {
              if (xAxisMode !== 'time') return
              const { left, width } = u.select
              if (width > 0 && onTimeRangeSelectRef.current) {
                const fromTime = u.posToVal(left, 'x')
                const toTime = u.posToVal(left + width, 'x')
                const fromDate = new Date(fromTime * 1000)
                const toDate = new Date(toTime * 1000)
                u.setSelect({ left: 0, width: 0, top: 0, height: 0 }, false)
                onTimeRangeSelectRef.current(fromDate, toDate)
              }
            },
          ],
        },
        legend: { show: false },
      }

      const chartContainer = containerRef.current.querySelector('.chart-inner') as HTMLElement
      if (chartContainer) {
        chartRef.current = new uPlot(opts, [xValues, yValues], chartContainer)
      }
    }

    return () => {
      if (chartRef.current) {
        chartRef.current.destroy()
        chartRef.current = null
      }
    }
    // Note: dimensions intentionally excluded - handled by separate resize effect
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data, normalizedSeries, title, unit, primaryUnit, createTooltipPlugin, createMultiSeriesTooltipPlugin, stats, adaptiveTimeUnit, adaptiveSizeUnit, adaptiveBitUnit, scaleMode, chartType, xAxisMode, xLabels, yColumnName, isMultiSeries, unitScaleInfo, seriesVisibility])

  // Resize chart without recreating when dimensions change
  useEffect(() => {
    if (chartRef.current && dimensions.width > 0 && dimensions.height > 0) {
      chartRef.current.setSize({ width: dimensions.width, height: dimensions.height })
    }
  }, [dimensions])

  // Legend click handler (Grafana-style)
  const handleLegendClick = useCallback((idx: number, ctrlKey: boolean) => {
    setSeriesVisibility(prev => {
      const current = prev ?? new Array(effectiveSeriesCount).fill(true)

      if (ctrlKey) {
        // Ctrl+Click: toggle single series
        const next = [...current]
        next[idx] = !next[idx]
        setIsolatedSeries(null)
        return next
      }

      // Click: isolate this series (or restore all if already isolated)
      if (isolatedSeries === idx) {
        // Already isolated — restore all
        setIsolatedSeries(null)
        return null // null = all visible
      }

      // Isolate: show only this series
      const next = new Array(effectiveSeriesCount).fill(false)
      next[idx] = true
      setIsolatedSeries(idx)
      return next
    })
  }, [effectiveSeriesCount, isolatedSeries])

  // Build display title with column names if available
  const displayTitle = title || yColumnName || ''
  const xAxisLabel = xColumnName || (xAxisMode === 'time' ? 'Time' : 'X')

  const showMultiSeriesHeader = isMultiSeries && normalizedSeries.length > 1
  const totalDataCount = normalizedSeries.reduce((sum, s) => sum + s.data.length, 0)

  return (
    <div className="flex flex-col h-full bg-app-panel border border-theme-border rounded-lg">
      {/* Chart header */}
      <div className="relative z-10 flex justify-between items-center px-4 py-3 border-b border-theme-border" onClick={(e) => e.stopPropagation()}>
        {showMultiSeriesHeader ? (
          <div className="flex items-center gap-3">
            {normalizedSeries.map((s, i) => {
              const color = SERIES_COLORS[i % SERIES_COLORS.length]
              const isVisible = seriesVisibility ? seriesVisibility[i] : true
              return (
                <button
                  key={i}
                  className={`flex items-center gap-1.5 px-1.5 py-0.5 rounded text-xs cursor-pointer transition-all ${
                    isVisible ? '' : 'opacity-35'
                  } hover:bg-white/5`}
                  onClick={(e) => handleLegendClick(i, e.ctrlKey || e.metaKey)}
                  title="Click to isolate, Ctrl+Click to toggle"
                >
                  <div className="w-2.5 h-[3px] rounded-sm" style={{ background: color }} />
                  <span className={isVisible ? 'text-theme-text-secondary' : 'text-theme-text-muted'}>
                    {s.label}
                  </span>
                </button>
              )
            })}
          </div>
        ) : displayTitle ? (
          <div className="text-base font-medium text-theme-text-primary">
            {displayTitle}{displayUnit && <span className="text-theme-text-muted font-normal"> ({displayUnit})</span>}
            {xAxisMode !== 'time' && xAxisLabel && (
              <span className="text-theme-text-muted font-normal text-sm ml-2">vs {xAxisLabel}</span>
            )}
          </div>
        ) : (
          <div />
        )}
        <div className="flex items-center gap-4 text-xs text-theme-text-muted">
          {!showMultiSeriesHeader && displayTitle && (
            <div className="flex items-center gap-1.5">
              <div className="w-3 h-0.5 bg-chart-line rounded" />
              <span>{displayTitle}</span>
            </div>
          )}
          {!showMultiSeriesHeader && (
            <>
              <div>
                min: <span className="text-theme-text-secondary">{formatStatValue(stats.min, primaryUnit)}</span>
              </div>
              <div>
                p99: <span className="text-theme-text-secondary">{formatStatValue(stats.p99, primaryUnit)}</span>
              </div>
              <div>
                max: <span className="text-theme-text-secondary">{formatStatValue(stats.max, primaryUnit)}</span>
              </div>
              <div>
                avg: <span className="text-theme-text-secondary">{formatStatValue(stats.avg, primaryUnit)}</span>
              </div>
              <div>
                count: <span className="text-theme-text-secondary">{(data?.length ?? totalDataCount).toLocaleString()}</span>
              </div>
            </>
          )}
          <div className="flex border border-theme-border rounded overflow-hidden" title="Chart display style">
            <button
              onClick={() => setChartType('line')}
              className={`px-2 py-0.5 text-[11px] transition-colors ${
                chartType === 'line'
                  ? 'bg-accent text-white'
                  : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-white/5'
              }`}
            >
              Line
            </button>
            <button
              onClick={() => setChartType('bar')}
              className={`px-2 py-0.5 text-[11px] border-l border-theme-border transition-colors ${
                chartType === 'bar'
                  ? 'bg-accent text-white'
                  : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-white/5'
              }`}
            >
              Bar
            </button>
          </div>
          <div className="flex border border-theme-border rounded overflow-hidden" title="P99: hides outliers &#10;Max: shows all data">
            <button
              onClick={() => setScaleMode('p99')}
              className={`px-2 py-0.5 text-[11px] transition-colors ${
                scaleMode === 'p99'
                  ? 'bg-accent text-white'
                  : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-white/5'
              }`}
            >
              P99
            </button>
            <button
              onClick={() => setScaleMode('max')}
              className={`px-2 py-0.5 text-[11px] border-l border-theme-border transition-colors ${
                scaleMode === 'max'
                  ? 'bg-accent text-white'
                  : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-white/5'
              }`}
            >
              Max
            </button>
          </div>
        </div>
      </div>

      {/* Chart container - min-h-0 prevents flex content from setting min-height */}
      <div ref={containerRef} className="flex-1 min-h-0 p-4 flex items-center justify-center">
        <div className="chart-inner" />
      </div>
    </div>
  )
}

// Re-export TimeSeriesChart as an alias for backwards compatibility
// This allows existing code that uses TimeSeriesChart with time/value data to continue working
export interface TimeSeriesChartProps {
  data: { time: number; value: number }[]
  title: string
  unit: string
  scaleMode?: ScaleMode
  onScaleModeChange?: (mode: ScaleMode) => void
  onTimeRangeSelect?: (from: Date, to: Date) => void
  onWidthChange?: (width: number) => void
  onAxisBoundsChange?: (bounds: ChartAxisBounds) => void
}

export function TimeSeriesChart({
  data,
  title,
  unit,
  scaleMode,
  onScaleModeChange,
  onTimeRangeSelect,
  onWidthChange,
  onAxisBoundsChange,
}: TimeSeriesChartProps) {
  // Convert time/value format to x/y format
  const xyData = useMemo(() => data.map((d) => ({ x: d.time, y: d.value })), [data])

  return (
    <XYChart
      data={xyData}
      xAxisMode="time"
      title={title}
      unit={unit}
      scaleMode={scaleMode}
      onScaleModeChange={onScaleModeChange}
      onTimeRangeSelect={onTimeRangeSelect}
      onWidthChange={onWidthChange}
      onAxisBoundsChange={onAxisBoundsChange}
    />
  )
}
