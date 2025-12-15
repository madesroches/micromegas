import { useEffect, useRef, useState, useCallback, useMemo } from 'react'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'

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

const UNIT_ABBREVIATIONS: Record<string, string> = {
  seconds: 's',
  milliseconds: 'ms',
  microseconds: 'µs',
  nanoseconds: 'ns',
  minutes: 'min',
  hours: 'h',
}

function formatValue(value: number, unit: string, abbreviated = false): string {
  const displayUnit = abbreviated ? (UNIT_ABBREVIATIONS[unit] ?? unit) : unit
  if (unit === 'bytes') {
    if (value >= 1e9) return (value / 1e9).toFixed(1) + ' GB'
    if (value >= 1e6) return (value / 1e6).toFixed(1) + ' MB'
    if (value >= 1e3) return (value / 1e3).toFixed(1) + ' KB'
    return value.toFixed(0) + ' B'
  }
  if (unit === 'percent') return value.toFixed(1) + '%'
  if (unit === 'count') return Math.round(value).toLocaleString()
  return value.toFixed(2) + ' ' + displayUnit
}

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

  // Calculate stats
  const stats = useMemo(() => ({
    min: data.length > 0 ? Math.min(...data.map((d) => d.value)) : 0,
    max: data.length > 0 ? Math.max(...data.map((d) => d.value)) : 0,
    avg: data.length > 0 ? data.reduce((sum, d) => sum + d.value, 0) / data.length : 0,
  }), [data])

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

  // Tooltip plugin
  const createTooltipPlugin = useCallback(
    (chartUnit: string): uPlot.Plugin => {
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
            tooltipValue.textContent = formatValue(value, chartUnit)

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
    const times = data.map((d) => d.time / 1000) // uPlot uses seconds
    const values = data.map((d) => d.value)

    const opts: uPlot.Options = {
      width: dimensions.width,
      height: dimensions.height,
      plugins: [createTooltipPlugin(unit)],
      scales: {
        x: { time: true },
        y: { auto: true },
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
          values: (_u: uPlot, vals: number[]) => vals.map((v) => formatValue(v, unit, true)),
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
  }, [data, dimensions, title, unit, createTooltipPlugin, onTimeRangeSelect, onAxisBoundsChange])

  return (
    <div className="flex flex-col h-full bg-app-panel border border-theme-border rounded-lg">
      {/* Chart header */}
      <div className="flex justify-between items-center px-4 py-3 border-b border-theme-border">
        <div className="text-base font-medium text-theme-text-primary">
          {title} <span className="text-theme-text-muted font-normal">({unit})</span>
        </div>
        <div className="flex items-center gap-4 text-xs text-theme-text-muted">
          <div className="flex items-center gap-1.5">
            <div className="w-3 h-0.5 bg-chart-line rounded" />
            <span>{title}</span>
          </div>
          <div>
            min: <span className="text-theme-text-secondary">{formatValue(stats.min, unit)}</span>
          </div>
          <div>
            max: <span className="text-theme-text-secondary">{formatValue(stats.max, unit)}</span>
          </div>
          <div>
            avg: <span className="text-theme-text-secondary">{formatValue(stats.avg, unit)}</span>
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
