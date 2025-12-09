'use client'

import { ThreadCoverage } from '@/types'
import { ChartAxisBounds } from './TimeSeriesChart'

interface ThreadCoverageTimelineProps {
  threads: ThreadCoverage[]
  timeRange: { from: number; to: number }
  axisBounds?: ChartAxisBounds | null
}

export function ThreadCoverageTimeline({
  threads,
  timeRange,
  axisBounds,
}: ThreadCoverageTimelineProps) {
  const duration = timeRange.to - timeRange.from

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
                className="h-6 relative bg-app-bg rounded"
                style={{ width: plotWidth ?? '100%' }}
              >
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
                      className="absolute top-1 bottom-1 bg-chart-line rounded-sm opacity-80 hover:opacity-100 transition-opacity"
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
