'use client'

interface ThreadSegment {
  begin: number
  end: number
}

interface ThreadCoverage {
  streamId: string
  threadName: string
  segments: ThreadSegment[]
}

interface ThreadCoverageTimelineProps {
  threads: ThreadCoverage[]
  timeRange: { from: number; to: number }
}

export function ThreadCoverageTimeline({
  threads,
  timeRange,
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

  return (
    <div className="bg-app-panel border border-theme-border rounded-lg overflow-hidden">
      {/* Header */}
      <div className="px-4 py-3 border-b border-theme-border">
        <div className="text-base font-medium text-theme-text-primary">Thread Coverage</div>
        <div className="text-xs text-theme-text-muted mt-1">
          {threads.length} thread{threads.length !== 1 ? 's' : ''} with trace data
        </div>
      </div>

      {/* Timeline */}
      <div className="divide-y divide-theme-border">
        {threads.map((thread) => (
          <div key={thread.streamId} className="flex items-center">
            {/* Thread name */}
            <div
              className="w-32 flex-shrink-0 px-4 py-2 text-sm text-theme-text-secondary truncate border-r border-theme-border"
              title={thread.threadName}
            >
              {thread.threadName}
            </div>

            {/* Timeline bar area */}
            <div className="flex-1 h-8 relative bg-app-bg mx-2 my-1 rounded">
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

      {/* Empty state */}
      {threads.length === 0 && (
        <div className="px-4 py-8 text-center text-sm text-theme-text-muted">
          No thread trace data available
        </div>
      )}
    </div>
  )
}
