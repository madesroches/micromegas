'use client'

import { useState, useRef, useCallback } from 'react'
import { ThreadCoverage } from '@/types'
import { ChartAxisBounds } from './TimeSeriesChart'

interface ThreadCoverageTimelineProps {
  threads: ThreadCoverage[]
  timeRange: { from: number; to: number }
  axisBounds?: ChartAxisBounds | null
  onTimeRangeSelect?: (from: Date, to: Date) => void
}

export function ThreadCoverageTimeline({
  threads,
  timeRange,
  axisBounds,
  onTimeRangeSelect,
}: ThreadCoverageTimelineProps) {
  const duration = timeRange.to - timeRange.from
  const containerRef = useRef<HTMLDivElement>(null)
  const [selection, setSelection] = useState<{ startX: number; currentX: number } | null>(null)
  const [isDragging, setIsDragging] = useState(false)

  // Calculate position as percentage
  const toPercent = (time: number) => {
    return ((time - timeRange.from) / duration) * 100
  }

  // Clamp values to visible range
  const clamp = (value: number, min: number, max: number) => {
    return Math.max(min, Math.min(max, value))
  }

  // Use axis bounds if available, otherwise fall back to defaults
  const leftOffset = axisBounds ? axisBounds.left : 60 // Default Y-axis width
  const plotWidth = axisBounds ? axisBounds.width : undefined

  // Convert pixel position to time
  const pixelToTime = useCallback(
    (pixelX: number, containerWidth: number): number => {
      const ratio = pixelX / containerWidth
      return timeRange.from + ratio * duration
    },
    [timeRange.from, duration]
  )

  // Get pixel position relative to the timeline bar
  const getRelativeX = useCallback((e: React.MouseEvent, element: HTMLElement): number => {
    const rect = element.getBoundingClientRect()
    return Math.max(0, Math.min(e.clientX - rect.left, rect.width))
  }, [])

  const handleMouseDown = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!onTimeRangeSelect) return
      const target = e.currentTarget
      const x = getRelativeX(e, target)
      setSelection({ startX: x, currentX: x })
      setIsDragging(true)
      e.preventDefault()
    },
    [onTimeRangeSelect, getRelativeX]
  )

  const handleMouseMove = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!isDragging || !selection) return
      const target = e.currentTarget
      const x = getRelativeX(e, target)
      setSelection((prev) => (prev ? { ...prev, currentX: x } : null))
    },
    [isDragging, selection, getRelativeX]
  )

  const handleMouseUp = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!isDragging || !selection || !onTimeRangeSelect) {
        setSelection(null)
        setIsDragging(false)
        return
      }

      const target = e.currentTarget
      const containerWidth = target.getBoundingClientRect().width
      const minX = Math.min(selection.startX, selection.currentX)
      const maxX = Math.max(selection.startX, selection.currentX)

      // Only trigger if selection is meaningful (at least 5 pixels)
      if (maxX - minX > 5) {
        const fromTime = pixelToTime(minX, containerWidth)
        const toTime = pixelToTime(maxX, containerWidth)
        onTimeRangeSelect(new Date(fromTime), new Date(toTime))
      }

      setSelection(null)
      setIsDragging(false)
    },
    [isDragging, selection, onTimeRangeSelect, pixelToTime]
  )

  const handleMouseLeave = useCallback(() => {
    if (isDragging) {
      setSelection(null)
      setIsDragging(false)
    }
  }, [isDragging])

  // Calculate selection overlay position
  const getSelectionStyle = useCallback(() => {
    if (!selection) return { display: 'none' }
    const left = Math.min(selection.startX, selection.currentX)
    const width = Math.abs(selection.currentX - selection.startX)
    return {
      left: `${left}px`,
      width: `${width}px`,
      display: width > 2 ? 'block' : 'none',
    }
  }, [selection])

  return (
    <div className="bg-app-panel border border-theme-border rounded-lg overflow-hidden">
      {/* Header */}
      <div className="px-4 py-3 border-b border-theme-border">
        <div className="text-base font-medium text-theme-text-primary">Thread Coverage</div>
        <div className="text-xs text-theme-text-muted mt-1">
          {threads.length} thread{threads.length !== 1 ? 's' : ''} with trace data
        </div>
      </div>

      {/* Timeline - wrapped in padding to match chart container */}
      <div className="p-4">
        <div className="divide-y divide-theme-border/50">
          {threads.map((thread) => (
            <div key={thread.streamId} className="flex items-center h-8">
              {/* Spacer to align with chart Y-axis area */}
              <div className="flex-shrink-0" style={{ width: leftOffset }} />

              {/* Timeline bar area - matches chart plot area */}
              <div
                ref={containerRef}
                className={`h-6 relative bg-app-bg rounded ${onTimeRangeSelect ? 'cursor-crosshair' : ''}`}
                style={{ width: plotWidth ?? '100%' }}
                onMouseDown={handleMouseDown}
                onMouseMove={handleMouseMove}
                onMouseUp={handleMouseUp}
                onMouseLeave={handleMouseLeave}
              >
                {/* Selection overlay - matches uPlot's selection style */}
                {selection && (
                  <div
                    className="absolute top-0 bottom-0 pointer-events-none z-20"
                    style={{
                      ...getSelectionStyle(),
                      background: 'var(--chart-selection)',
                      borderLeft: '2px solid var(--chart-selection-border)',
                      borderRight: '2px solid var(--chart-selection-border)',
                    }}
                  />
                )}

                {/* Thread name overlay */}
                <div
                  className="absolute inset-y-0 left-1 flex items-center z-10 pointer-events-none"
                  title={thread.threadName}
                >
                  <span className="text-xs font-medium px-1 rounded text-brand-blue bg-black/60">
                    {thread.threadName}
                  </span>
                </div>

                {/* Coverage segments */}
                {thread.segments.map((segment, idx) => {
                  const startPercent = clamp(toPercent(segment.begin), 0, 100)
                  const endPercent = clamp(toPercent(segment.end), 0, 100)
                  const widthPercent = endPercent - startPercent

                  // Skip segments entirely outside the visible range
                  if (widthPercent <= 0) return null

                  return (
                    <div
                      key={idx}
                      className="absolute top-1 bottom-1 bg-chart-line rounded-sm opacity-80 hover:opacity-100 transition-opacity pointer-events-none"
                      style={{
                        left: `${startPercent}%`,
                        width: `${Math.max(widthPercent, 0.5)}%`, // Min width for visibility
                      }}
                      title={`${new Date(segment.begin).toLocaleTimeString()} - ${new Date(segment.end).toLocaleTimeString()}`}
                    />
                  )
                })}
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* Empty state */}
      {threads.length === 0 && (
        <div className="px-4 py-8 text-center text-sm text-theme-text-muted">
          No thread trace data available
        </div>
      )}
    </div>
  )
}
