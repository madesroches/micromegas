/**
 * Thread-coverage panel for the Performance Analysis page.
 *
 * Owns the per-thread CPU block coverage and runs both the coverage query and
 * the trace event-count query (they share a fetch trigger). The thread list is
 * local; the event count is lifted to the page because it is shown in the
 * toolbar. Renders the bottom coverage timeline and registers its loader into
 * the page-held ref so the gate re-triggers it exactly as before (#1089).
 */
import { useCallback, useState, type Dispatch, type MutableRefObject, type SetStateAction } from 'react'
import { executeStreamQuery } from '@/lib/arrow-stream'
import { timestampToMs } from '@/lib/arrow-utils'
import { ThreadCoverageTimeline } from '@/components/ThreadCoverageTimeline'
import { ChartAxisBounds } from '@/components/XYChart'
import { ThreadCoverage } from '@/types'
import { THREAD_COVERAGE_SQL, TRACE_EVENTS_COUNT_SQL } from './queries'

interface ThreadCoveragePanelProps {
  processId: string
  timeRange: { begin: string; end: string }
  dataSource?: string
  chartTimeRange: { from: number; to: number } | null
  chartAxisBounds: ChartAxisBounds | null
  onTimeRangeSelect: (from: Date, to: Date) => void
  setTraceEventCount: Dispatch<SetStateAction<number | null>>
  setTraceEventCountLoading: Dispatch<SetStateAction<boolean>>
  /** Page registers this loader so the time-range gate can re-trigger it. */
  loadRef: MutableRefObject<(() => Promise<void>) | null>
}

export function ThreadCoveragePanel({
  processId,
  timeRange,
  dataSource,
  chartTimeRange,
  chartAxisBounds,
  onTimeRangeSelect,
  setTraceEventCount,
  setTraceEventCountLoading,
  loadRef,
}: ThreadCoveragePanelProps) {
  const [threadCoverage, setThreadCoverage] = useState<ThreadCoverage[]>([])

  const loadThreadCoverage = useCallback(async () => {
    if (!processId) return

    // Load thread coverage
    try {
      const { batches, error } = await executeStreamQuery({
        sql: THREAD_COVERAGE_SQL,
        params: { process_id: processId },
        begin: timeRange.begin,
        end: timeRange.end,
        dataSource,
      })

      if (error) {
        console.error('Failed to fetch thread coverage:', error.message)
        setThreadCoverage([])
      } else {
        const threadMap = new Map<string, ThreadCoverage>()

        for (const batch of batches) {
          for (let i = 0; i < batch.numRows; i++) {
            const row = batch.get(i)
            if (row) {
              const streamId = String(row.stream_id ?? '')
              const threadName = String(row.thread_name ?? 'unknown')
              const beginTime = timestampToMs(row.begin_time)
              const endTime = timestampToMs(row.end_time)

              if (!threadMap.has(streamId)) {
                threadMap.set(streamId, {
                  streamId,
                  threadName,
                  segments: [],
                })
              }
              threadMap.get(streamId)!.segments.push({ begin: beginTime, end: endTime })
            }
          }
        }

        const threads = Array.from(threadMap.values())
        threads.sort((a, b) => a.threadName.localeCompare(b.threadName))
        for (const thread of threads) {
          thread.segments.sort((a, b) => a.begin - b.begin)
        }

        setThreadCoverage(threads)
      }
    } catch (err) {
      console.error('Failed to fetch thread coverage:', err)
      setThreadCoverage([])
    }

    // Load trace event count
    setTraceEventCountLoading(true)
    try {
      const { batches, error } = await executeStreamQuery({
        sql: TRACE_EVENTS_COUNT_SQL,
        params: { process_id: processId },
        begin: timeRange.begin,
        end: timeRange.end,
        dataSource,
      })

      if (error) {
        console.error('Failed to fetch trace event count:', error.message)
        setTraceEventCount(0)
      } else {
        let eventCount = 0
        for (const batch of batches) {
          if (batch.numRows > 0) {
            const row = batch.get(0)
            if (row && row.event_count != null) {
              eventCount = Number(row.event_count)
            }
          }
        }
        setTraceEventCount(eventCount)
      }
    } catch (err) {
      console.error('Failed to fetch trace event count:', err)
      setTraceEventCount(0)
    } finally {
      setTraceEventCountLoading(false)
    }
  }, [processId, timeRange.begin, timeRange.end, dataSource, setTraceEventCount, setTraceEventCountLoading])

  // Always expose the latest loader to the page without re-running effects.
  loadRef.current = loadThreadCoverage

  if (!chartTimeRange || threadCoverage.length === 0) return null

  return (
    <ThreadCoverageTimeline
      threads={threadCoverage}
      timeRange={chartTimeRange}
      axisBounds={chartAxisBounds}
      onTimeRangeSelect={onTimeRangeSelect}
    />
  )
}
