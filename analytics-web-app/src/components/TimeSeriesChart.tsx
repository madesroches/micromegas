import { useEffect, useRef, useState, useCallback, useMemo } from 'react'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'
import {
  isTimeUnit,
  getAdaptiveTimeUnit,
  formatAdaptiveTime,
  formatTimeValue,
  type AdaptiveTimeUnit,
  type TimeUnit,
} from '@/lib/time-units'

export interface ChartAxisBounds {
  left: number // Left padding (Y-axis width)
  width: number // Plot area width
}

interface TimeSeriesChartProps {
  data: { time: number; value: number }[]
  title: string
  unit: string
  onTimeRangeSelect?: (from: Date, to: Date) => void
  onWidthChange?: (width: number) => void
  onAxisBoundsChange?: (bounds: ChartAxisBounds) => void
}

function formatValue(
  value: number,
  unit: string,
  abbreviated = false,
  adaptiveTimeUnit?: AdaptiveTimeUnit
): string {
  // Use adaptive formatting for time units
  if (adaptiveTimeUnit && isTimeUnit(unit)) {
    return formatAdaptiveTime(value, adaptiveTimeUnit, abbreviated)
  }

  if (unit === 'bytes') {
    if (value >= 1e9) return (value / 1e9).toFixed(1) + ' GB'
    if (value >= 1e6) return (value / 1e6).toFixed(1) + ' MB'
    if (value >= 1e3) return (value / 1e3).toFixed(1) + ' KB'
    return value.toFixed(0) + ' B'
  }
  if (unit === 'percent') return value.toFixed(1) + '%'
  if (unit === 'count') return Math.round(value).toLocaleString()
  return value.toFixed(2) + ' ' + unit
}

// Format a stat value - for time units, each value picks its own best unit
function formatStatValue(value: number, unit: string): string {
  if (isTimeUnit(unit)) {
    return formatTimeValue(value, unit as TimeUnit, false)
  }
  return formatValue(value, unit, false)
}

type ScaleMode = 'p99' | 'max'

export function TimeSeriesChart({
  data,
  title,
  unit,
  onTimeRangeSelect,
  onWidthChange,
  onAxisBoundsChange,
}: TimeSeriesChartProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const chartRef = useRef<uPlot | null>(null)
  const [dimensions, setDimensions] = useState({ width: 800, height: 300 })
  const [scaleMode, setScaleMode] = useState<ScaleMode>('p99')

  // Calculate stats including percentile for scaling
  const stats = useMemo(() => {
    if (data.length === 0) {
      return { min: 0, max: 0, avg: 0, p99: 0 }
    }
    const values = data.map((d) => d.value)
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
    return getAdaptiveTimeUnit(stats.p99, unit as TimeUnit)
  }, [unit, stats.p99])

  // Display unit for the header (adaptive for time, original for others)
  const displayUnit = adaptiveTimeUnit ? adaptiveTimeUnit.unit : unit

  // Handle resize
  useEffect(() => {
    const updateDimensions = () => {
      if (containerRef.current) {
        const rect = containerRef.current.getBoundingClientRect()
        const newWidth = Math.max(400, rect.width - 32)
        setDimensions({
          width: newWidth,
          height: Math.max(250, rect.height - 32),
        })
        onWidthChange?.(newWidth)
      }
    }

    updateDimensions()
    const resizeObserver = new ResizeObserver(updateDimensions)
    if (containerRef.current) {
      resizeObserver.observe(containerRef.current)
    }

    return () => resizeObserver.disconnect()
  }, [onWidthChange])

  // Tooltip plugin - values are in display unit, convert back to original for formatting
  const createTooltipPlugin = useCallback(
    (originalUnit: string, conversionFactor: number): uPlot.Plugin => {
      let tooltip: HTMLDivElement
      let tooltipTime: HTMLDivElement
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
            tooltipTime = tooltip.children[0] as HTMLDivElement
            tooltipValue = tooltip.children[1] as HTMLDivElement
          },
          setCursor: (u: uPlot) => {
            const { idx, left, top } = u.cursor
            if (idx == null || left == null || top == null || left < 0 || top < 0) {
              tooltip.style.display = 'none'
              return
            }

            const time = u.data[0][idx]
            const value = u.data[1][idx]

            if (time == null || value == null) {
              tooltip.style.display = 'none'
              return
            }

            const date = new Date(time * 1000)
            const timeStr =
              date.toLocaleTimeString('en-US', {
                hour: '2-digit',
                minute: '2-digit',
                second: '2-digit',
                hour12: false,
              }) +
              '.' +
              String(date.getMilliseconds()).padStart(3, '0')

            tooltipTime.textContent = timeStr

            // Convert back to original unit and pick best unit for display
            const originalValue = value / conversionFactor
            if (isTimeUnit(originalUnit)) {
              tooltipValue.textContent = formatTimeValue(originalValue, originalUnit as TimeUnit)
            } else {
              tooltipValue.textContent = formatStatValue(originalValue, originalUnit)
            }

            tooltip.style.left = left + 10 + 'px'
            tooltip.style.top = Math.max(0, top - 60) + 'px'
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
    // For time units, convert values to the display unit so uPlot generates correct ticks
    const conversionFactor = adaptiveTimeUnit?.conversionFactor ?? 1
    const times = data.map((d) => d.time / 1000) // uPlot uses seconds for X axis
    const values = data.map((d) => d.value * conversionFactor)

    // Convert stats to display unit for scale range
    const _displayMin = stats.min * conversionFactor
    const displayP99 = stats.p99 * conversionFactor
    const displayMax = stats.max * conversionFactor

    const yAxisUnit = adaptiveTimeUnit?.abbrev ?? unit

    const opts: uPlot.Options = {
      width: dimensions.width,
      height: dimensions.height,
      plugins: [createTooltipPlugin(unit, conversionFactor)],
      scales: {
        x: { time: true },
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
        {
          stroke: '#6a6a7a',
          grid: { stroke: '#2a2a35', width: 1 },
          ticks: { stroke: '#2a2a35', width: 1 },
          font: '11px -apple-system, BlinkMacSystemFont, sans-serif',
        },
        {
          stroke: '#6a6a7a',
          grid: { stroke: '#2a2a35', width: 1 },
          ticks: { stroke: '#2a2a35', width: 1 },
          font: '11px -apple-system, BlinkMacSystemFont, sans-serif',
          size: 70, // Ensure enough space for labels
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
          label: title,
          stroke: '#bf360c',
          width: 2,
          fill: 'rgba(191, 54, 12, 0.1)',
          points: { show: false },
        },
      ],
      cursor: {
        show: true,
        x: true,
        y: true,
        drag: {
          x: true,
          y: false,
          setScale: false, // Don't auto-zoom, we'll handle it via callback
        },
      },
      hooks: {
        ready: [
          (u: uPlot) => {
            // Report axis bounds after chart layout is complete
            if (onAxisBoundsChange) {
              onAxisBoundsChange({
                left: u.bbox.left / devicePixelRatio,
                width: u.bbox.width / devicePixelRatio,
              })
            }
          },
        ],
        setSize: [
          (u: uPlot) => {
            // Report updated axis bounds after resize
            if (onAxisBoundsChange) {
              onAxisBoundsChange({
                left: u.bbox.left / devicePixelRatio,
                width: u.bbox.width / devicePixelRatio,
              })
            }
          },
        ],
        setSelect: [
          (u: uPlot) => {
            const { left, width } = u.select
            if (width > 0 && onTimeRangeSelect) {
              // Convert pixel positions to time values
              const fromTime = u.posToVal(left, 'x')
              const toTime = u.posToVal(left + width, 'x')
              // uPlot uses seconds, convert to Date
              const fromDate = new Date(fromTime * 1000)
              const toDate = new Date(toTime * 1000)
              // Clear the selection visual
              u.setSelect({ left: 0, width: 0, top: 0, height: 0 }, false)
              // Call the callback
              onTimeRangeSelect(fromDate, toDate)
            }
          },
        ],
      },
      legend: { show: false },
    }

    const chartContainer = containerRef.current.querySelector('.chart-inner') as HTMLElement
    if (chartContainer) {
      chartRef.current = new uPlot(opts, [times, values], chartContainer)
    }

    return () => {
      if (chartRef.current) {
        chartRef.current.destroy()
        chartRef.current = null
      }
    }
  }, [data, dimensions, title, unit, createTooltipPlugin, onTimeRangeSelect, onAxisBoundsChange, stats, adaptiveTimeUnit, scaleMode])

  return (
    <div className="flex flex-col h-full bg-app-panel border border-theme-border rounded-lg">
      {/* Chart header */}
      <div className="flex justify-between items-center px-4 py-3 border-b border-theme-border">
        <div className="text-base font-medium text-theme-text-primary">
          {title} <span className="text-theme-text-muted font-normal">({displayUnit})</span>
        </div>
        <div className="flex items-center gap-4 text-xs text-theme-text-muted">
          <div className="flex items-center gap-1.5">
            <div className="w-3 h-0.5 bg-chart-line rounded" />
            <span>{title}</span>
          </div>
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

      {/* Chart container */}
      <div ref={containerRef} className="flex-1 p-4 flex items-center justify-center">
        <div className="chart-inner" />
      </div>
    </div>
  )
}
