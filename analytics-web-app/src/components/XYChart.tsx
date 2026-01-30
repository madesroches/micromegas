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
import { normalizeUnit, isSizeUnit, getAdaptiveSizeUnit } from '@/lib/units'

export interface ChartAxisBounds {
  left: number // Left padding (Y-axis width)
  width: number // Plot area width
}

export type ScaleMode = 'p99' | 'max'

export type ChartType = 'line' | 'bar'

export type XAxisMode = 'time' | 'numeric' | 'categorical'

interface XYChartProps {
  data: { x: number; y: number }[] // categorical: x is index into xLabels
  xAxisMode: XAxisMode // required, determined by extractChartData
  xLabels?: string[] // for categorical mode - the actual string labels
  xColumnName?: string
  yColumnName?: string
  title?: string
  unit?: string
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

export function XYChart({
  data,
  xAxisMode,
  xLabels,
  xColumnName,
  yColumnName,
  title = '',
  unit = '',
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

  // Calculate stats including percentile for scaling
  const stats = useMemo(() => {
    if (data.length === 0) {
      return { min: 0, max: 0, avg: 0, p99: 0 }
    }
    const values = data.map((d) => d.y)
    const sorted = [...values].sort((a, b) => a - b)
    const p99Index = Math.floor(sorted.length * 0.99)
    return {
      min: sorted[0],
      max: sorted[sorted.length - 1],
      avg: values.reduce((sum, v) => sum + v, 0) / values.length,
      p99: sorted[Math.min(p99Index, sorted.length - 1)],
    }
  }, [data])

  // Calculate adaptive time unit based on p99 value
  const adaptiveTimeUnit = useMemo(() => {
    if (!isTimeUnit(unit) || stats.p99 === 0) {
      return undefined
    }
    return getAdaptiveTimeUnit(stats.p99, unit)
  }, [unit, stats.p99])

  // Calculate adaptive size unit based on p99 value
  const adaptiveSizeUnit = useMemo(() => {
    if (!isSizeUnit(unit) || stats.p99 === 0) {
      return undefined
    }
    return getAdaptiveSizeUnit(stats.p99, unit)
  }, [unit, stats.p99])

  // Display unit for the header (adaptive for time/size, original for others)
  const displayUnit = adaptiveTimeUnit?.unit ?? adaptiveSizeUnit?.unit ?? unit

  // Use ref for onWidthChange to avoid effect re-runs when callback identity changes
  const onWidthChangeRef = useRef(onWidthChange)
  onWidthChangeRef.current = onWidthChange

  // Track last reported width to avoid duplicate callbacks
  const lastReportedWidthRef = useRef<number | null>(null)

  // Measure container and update dimensions
  // This is called by ResizeObserver for internal sizing, but only reports
  // width changes to parent on user-initiated window resize events
  const measureContainer = useCallback(() => {
    if (containerRef.current) {
      const rect = containerRef.current.getBoundingClientRect()
      const newWidth = Math.round(Math.max(400, rect.width - 32))
      const newHeight = Math.round(Math.max(250, rect.height - 32))

      setDimensions((prev) => {
        if (prev.width === newWidth && prev.height === newHeight) {
          return prev
        }
        return { width: newWidth, height: newHeight }
      })

      return newWidth
    }
    return null
  }, [])

  // Handle initial mount measurement
  useEffect(() => {
    // Measure on mount and report initial width to parent
    const width = measureContainer()
    if (width !== null && lastReportedWidthRef.current !== width) {
      lastReportedWidthRef.current = width
      onWidthChangeRef.current?.(width)
    }

    // ResizeObserver handles internal dimension updates for chart rendering
    // but does NOT propagate to parent (avoids feedback loops from content reflows)
    const resizeObserver = new ResizeObserver(() => {
      measureContainer()
    })
    if (containerRef.current) {
      resizeObserver.observe(containerRef.current)
    }

    return () => resizeObserver.disconnect()
  }, [measureContainer])

  // Handle window resize - this is a user-initiated event, so we propagate width to parent
  // This allows parent to recalculate bin intervals when user resizes browser window
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

  // Tooltip plugin - values are in display unit, convert back to original for formatting
  const createTooltipPlugin = useCallback(
    (originalUnit: string, conversionFactor: number, mode: XAxisMode, labels?: string[]): uPlot.Plugin => {
      let tooltip: HTMLDivElement
      let tooltipX: HTMLDivElement
      let tooltipValue: HTMLDivElement

      return {
        hooks: {
          init: (u: uPlot) => {
            tooltip = document.createElement('div')
            tooltip.style.cssText = `
            position: absolute;
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
            u.over.appendChild(tooltip)
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

            // Position tooltip - flip below cursor if near top of chart
            const tooltipHeight = 60
            const flipThreshold = tooltipHeight + 10
            const posTop = top < flipThreshold ? top + 20 : top - tooltipHeight

            tooltip.style.left = left + 10 + 'px'
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

  // Create/update chart
  useEffect(() => {
    if (!containerRef.current || data.length === 0) return

    // Destroy previous chart
    if (chartRef.current) {
      chartRef.current.destroy()
      chartRef.current = null
    }

    // Transform data to uPlot format
    // For time/size units, convert values to the display unit so uPlot generates correct ticks
    const conversionFactor = adaptiveTimeUnit?.conversionFactor ?? adaptiveSizeUnit?.conversionFactor ?? 1

    // For time mode, convert ms to seconds for uPlot
    // For numeric/categorical, use x values directly
    const xValues = xAxisMode === 'time'
      ? data.map((d) => d.x / 1000) // ms to seconds for uPlot time axis
      : data.map((d) => d.x)
    const yValues = data.map((d) => d.y * conversionFactor)

    // Convert stats to display unit for scale range
    const _displayMin = stats.min * conversionFactor
    const displayP99 = stats.p99 * conversionFactor
    const displayMax = stats.max * conversionFactor

    const yAxisUnit = adaptiveTimeUnit?.abbrev ?? adaptiveSizeUnit?.abbrev ?? (unit === 'percent' ? '%' : unit)

    // Build X axis configuration based on mode
    const xAxisConfig: uPlot.Axis = {
      stroke: '#6a6a7a',
      grid: { stroke: '#2a2a35', width: 1 },
      ticks: { stroke: '#2a2a35', width: 1 },
      font: '11px -apple-system, BlinkMacSystemFont, sans-serif',
      size: 65, // Enough vertical space for two-line time labels
    }

    // For categorical mode, use custom values function for tick labels
    if (xAxisMode === 'categorical' && xLabels) {
      // Show only integer ticks at category positions
      xAxisConfig.incrs = [1]
      xAxisConfig.space = 60 // Minimum 60px between labels
      xAxisConfig.values = (_u: uPlot, vals: number[]) => {
        return vals.map((v) => {
          const idx = Math.round(v)
          if (idx >= 0 && idx < xLabels.length) {
            return xLabels[idx]
          }
          return ''
        })
      }
    } else if (xAxisMode === 'numeric') {
      // Ensure reasonable spacing between numeric ticks
      xAxisConfig.space = 60 // Minimum 60px between labels
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

    const opts: uPlot.Options = {
      width: dimensions.width,
      height: dimensions.height,
      plugins: [createTooltipPlugin(unit, conversionFactor, xAxisMode, xLabels)],
      // Use local timezone for time display (only applies to time mode)
      tzDate: xAxisMode === 'time' ? (ts: number) => new Date(ts * 1000) : undefined,
      scales: {
        x: { time: xAxisMode === 'time' },
        y: {
          // Scale based on user selection: p99 handles outliers gracefully, max shows all data
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
          size: 90, // Ensure enough space for labels like "90.0 percent"
          values: (_u: uPlot, vals: number[]) => {
            // Values are already in the display unit, just format them
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
          x: xAxisMode === 'time', // Only enable drag-to-select for time mode
          y: false,
          setScale: false, // Don't auto-zoom, we'll handle it via callback
        },
      },
      hooks: {
        ready: [
          (u: uPlot) => {
            // Report axis bounds after chart layout is complete
            onAxisBoundsChangeRef.current?.({
              left: u.bbox.left / devicePixelRatio,
              width: u.bbox.width / devicePixelRatio,
            })
          },
        ],
        setSize: [
          (u: uPlot) => {
            // Report updated axis bounds after resize
            onAxisBoundsChangeRef.current?.({
              left: u.bbox.left / devicePixelRatio,
              width: u.bbox.width / devicePixelRatio,
            })
          },
        ],
        setSelect: [
          (u: uPlot) => {
            // Only handle selection for time mode
            if (xAxisMode !== 'time') return

            const { left, width } = u.select
            if (width > 0 && onTimeRangeSelectRef.current) {
              // Convert pixel positions to time values
              const fromTime = u.posToVal(left, 'x')
              const toTime = u.posToVal(left + width, 'x')
              // uPlot uses seconds, convert to Date
              const fromDate = new Date(fromTime * 1000)
              const toDate = new Date(toTime * 1000)
              // Clear the selection visual
              u.setSelect({ left: 0, width: 0, top: 0, height: 0 }, false)
              // Call the callback
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

    return () => {
      if (chartRef.current) {
        chartRef.current.destroy()
        chartRef.current = null
      }
    }
    // Note: dimensions intentionally excluded - handled by separate resize effect
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data, title, unit, createTooltipPlugin, stats, adaptiveTimeUnit, adaptiveSizeUnit, scaleMode, chartType, xAxisMode, xLabels, yColumnName])

  // Resize chart without recreating when dimensions change
  useEffect(() => {
    if (chartRef.current && dimensions.width > 0 && dimensions.height > 0) {
      chartRef.current.setSize({ width: dimensions.width, height: dimensions.height })
    }
  }, [dimensions])

  // Build display title with column names if available
  const displayTitle = title || yColumnName || ''
  const xAxisLabel = xColumnName || (xAxisMode === 'time' ? 'Time' : 'X')

  return (
    <div className="flex flex-col h-full bg-app-panel border border-theme-border rounded-lg">
      {/* Chart header - relative z-10 ensures buttons are above chart canvas */}
      <div className="relative z-10 flex justify-between items-center px-4 py-3 border-b border-theme-border">
        {displayTitle ? (
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
          {displayTitle && (
            <div className="flex items-center gap-1.5">
              <div className="w-3 h-0.5 bg-chart-line rounded" />
              <span>{displayTitle}</span>
            </div>
          )}
          <div>
            min: <span className="text-theme-text-secondary">{formatStatValue(stats.min, unit)}</span>
          </div>
          <div>
            p99: <span className="text-theme-text-secondary">{formatStatValue(stats.p99, unit)}</span>
          </div>
          <div>
            max: <span className="text-theme-text-secondary">{formatStatValue(stats.max, unit)}</span>
          </div>
          <div>
            avg: <span className="text-theme-text-secondary">{formatStatValue(stats.avg, unit)}</span>
          </div>
          <div>
            count: <span className="text-theme-text-secondary">{data.length.toLocaleString()}</span>
          </div>
          <div className="relative group">
            <div className="flex border border-theme-border rounded overflow-hidden">
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
            <div className="absolute bottom-full right-0 mb-2 px-2 py-1.5 bg-app-panel border border-theme-border rounded text-[11px] text-theme-text-secondary opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none z-10 shadow-lg whitespace-nowrap">
              <div>Chart display style</div>
            </div>
          </div>
          <div className="relative group">
            <div className="flex border border-theme-border rounded overflow-hidden">
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
            <div className="absolute bottom-full right-0 mb-2 px-2 py-1.5 bg-app-panel border border-theme-border rounded text-[11px] text-theme-text-secondary opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none z-10 shadow-lg">
              <div>P99: hides outliers</div>
              <div>Max: shows all data</div>
            </div>
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
