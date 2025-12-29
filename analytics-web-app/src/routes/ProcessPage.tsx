import { Suspense, useState, useEffect, useCallback, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { AppLink } from '@/components/AppLink'
import { ArrowLeft, FileText, AlertCircle, BarChart2, Gauge } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { timestampToDate } from '@/lib/arrow-utils'
import { useTimeRange } from '@/hooks/useTimeRange'
import { formatDuration, formatDateTimeLocal } from '@/lib/time-range'

const PROCESS_SQL = `SELECT process_id, exe, start_time, last_update_time, computer, username, cpu_brand, distro
FROM processes
WHERE process_id = '$process_id'
LIMIT 1`

function formatLocalTime(timestamp: Date | null): string {
  if (!timestamp) return '—'
  return formatDateTimeLocal(timestamp)
}

const STATISTICS_SQL = `SELECT
  SUM(CASE WHEN array_has("streams.tags", 'log') THEN nb_objects ELSE 0 END) as log_entries,
  SUM(CASE WHEN array_has("streams.tags", 'metrics') THEN nb_objects ELSE 0 END) as measures,
  SUM(CASE WHEN array_has("streams.tags", 'cpu') THEN nb_objects ELSE 0 END) as trace_events,
  COUNT(DISTINCT CASE WHEN array_has("streams.tags", 'cpu') THEN stream_id ELSE NULL END) as thread_count
FROM blocks
WHERE process_id = '$process_id'`

const PROPERTIES_SQL = `SELECT jsonb_format_json(properties) as properties
FROM processes
WHERE process_id = '$process_id'
LIMIT 1`

const TWO_MINUTES_MS = 2 * 60 * 1000
const ONE_HOUR_MS = 60 * 60 * 1000

function computeProcessTimeRange(
  screenLoadTime: Date,
  startTime: Date | null,
  lastUpdateTime: Date | null
): { from: string; to: string } {
  // Parse last update time
  const lastUpdate = lastUpdateTime ?? screenLoadTime
  const timeSinceLastUpdate = screenLoadTime.getTime() - lastUpdate.getTime()

  // Determine end time
  let endTime: Date
  let toValue: string
  if (timeSinceLastUpdate < TWO_MINUTES_MS) {
    // Process is live - use "now" for live updates
    toValue = 'now'
    endTime = screenLoadTime
  } else {
    // Process is dead - use last update time
    endTime = lastUpdate
    toValue = lastUpdate.toISOString()
  }

  // Determine begin time
  const oneHourBeforeEnd = new Date(endTime.getTime() - ONE_HOUR_MS)
  const processStart = startTime ?? oneHourBeforeEnd

  // Use the more recent of: process start OR one hour before end
  let fromValue: string
  if (processStart.getTime() > oneHourBeforeEnd.getTime()) {
    fromValue = processStart.toISOString()
  } else {
    fromValue = oneHourBeforeEnd.toISOString()
  }

  return { from: fromValue, to: toValue }
}

interface ProcessRow {
  exe: string
  start_time: Date | null
  last_update_time: Date | null
  computer: string
  username: string
  cpu_brand: string
  distro: string
}

interface StatisticsRow {
  log_entries: number
  measures: number
  trace_events: number
  thread_count: number
}

function ProcessPageContent() {
  const [searchParams] = useSearchParams()
  const processId = searchParams.get('id')
  const { apiTimeRange } = useTimeRange()
  const [screenLoadTime] = useState(() => new Date())

  const [process, setProcess] = useState<ProcessRow | null>(null)
  const [statistics, setStatistics] = useState<StatisticsRow | null>(null)
  const [properties, setProperties] = useState<Record<string, string> | null>(null)
  const [propertiesError, setPropertiesError] = useState<string | null>(null)

  const processQuery = useStreamQuery()
  const statsQuery = useStreamQuery()
  const propertiesQuery = useStreamQuery()

  // Extract data from query results when complete
  useEffect(() => {
    if (processQuery.isComplete && !processQuery.error) {
      const table = processQuery.getTable()
      if (table && table.numRows > 0) {
        const row = table.get(0)
        if (row) {
          setProcess({
            exe: String(row.exe ?? ''),
            start_time: timestampToDate(row.start_time),
            last_update_time: timestampToDate(row.last_update_time),
            computer: String(row.computer ?? ''),
            username: String(row.username ?? ''),
            cpu_brand: String(row.cpu_brand ?? ''),
            distro: String(row.distro ?? ''),
          })
        }
      }
    }
  }, [processQuery.isComplete, processQuery.error])

  useEffect(() => {
    if (statsQuery.isComplete && !statsQuery.error) {
      const table = statsQuery.getTable()
      if (table && table.numRows > 0) {
        const row = table.get(0)
        if (row) {
          setStatistics({
            log_entries: Number(row.log_entries ?? 0),
            measures: Number(row.measures ?? 0),
            trace_events: Number(row.trace_events ?? 0),
            thread_count: Number(row.thread_count ?? 0),
          })
        }
      }
    }
  }, [statsQuery.isComplete, statsQuery.error])

  useEffect(() => {
    if (propertiesQuery.isComplete && !propertiesQuery.error) {
      const table = propertiesQuery.getTable()
      if (table && table.numRows > 0) {
        const row = table.get(0)
        if (row && row.properties) {
          try {
            const parsed = JSON.parse(String(row.properties))
            setProperties(parsed)
            setPropertiesError(null)
          } catch {
            setPropertiesError('Failed to parse properties')
          }
        } else {
          setProperties({})
        }
      } else {
        setProperties({})
      }
    } else if (propertiesQuery.error) {
      setPropertiesError(propertiesQuery.error.message)
    }
  }, [propertiesQuery.isComplete, propertiesQuery.error])

  const processExecuteRef = useRef(processQuery.execute)
  processExecuteRef.current = processQuery.execute
  const statsExecuteRef = useRef(statsQuery.execute)
  statsExecuteRef.current = statsQuery.execute
  const propertiesExecuteRef = useRef(propertiesQuery.execute)
  propertiesExecuteRef.current = propertiesQuery.execute

  const loadData = useCallback(() => {
    if (!processId) return
    processExecuteRef.current({
      sql: PROCESS_SQL,
      params: { process_id: processId },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    })
    statsExecuteRef.current({
      sql: STATISTICS_SQL,
      params: { process_id: processId },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    })
    propertiesExecuteRef.current({
      sql: PROPERTIES_SQL,
      params: { process_id: processId },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    })
  }, [processId, apiTimeRange])

  // Load data once on mount when we have a processId
  const hasLoadedRef = useRef(false)
  useEffect(() => {
    if (processId && !hasLoadedRef.current) {
      hasLoadedRef.current = true
      loadData()
    }
  }, [processId, loadData])

  const isLoading = processQuery.isStreaming || (!processQuery.isComplete && !processQuery.error)
  const statsError = statsQuery.error?.message ?? null

  if (!processId) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-app-panel border border-theme-border rounded-lg">
            <AlertCircle className="w-10 h-10 text-accent-error mb-3" />
            <p className="text-theme-text-secondary">No process ID provided</p>
            <AppLink href="/processes" className="text-accent-link hover:underline mt-2">
              Back to Processes
            </AppLink>
          </div>
        </div>
      </PageLayout>
    )
  }

  if (isLoading) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex items-center justify-center h-64 bg-app-panel border border-theme-border rounded-lg">
            <div className="flex items-center gap-3">
              <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
              <span className="text-theme-text-secondary">Loading process...</span>
            </div>
          </div>
        </div>
      </PageLayout>
    )
  }

  if (!process) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-app-panel border border-theme-border rounded-lg">
            <AlertCircle className="w-10 h-10 text-accent-error mb-3" />
            <p className="text-theme-text-secondary">Process not found</p>
            <AppLink href="/processes" className="text-accent-link hover:underline mt-2">
              Back to Processes
            </AppLink>
          </div>
        </div>
      </PageLayout>
    )
  }

  return (
    <PageLayout onRefresh={loadData}>
      <div className="p-6 max-w-6xl">
        {/* Back Link */}
        <AppLink
          href="/processes"
          className="inline-flex items-center gap-1.5 text-accent-link hover:underline text-sm mb-4"
        >
          <ArrowLeft className="w-3 h-3" />
          All Processes
        </AppLink>

        {/* Page Header */}
        <div className="flex items-start justify-between mb-8">
          <div>
            <h1 className="text-2xl font-semibold text-theme-text-primary">{String(process.exe ?? '')}</h1>
            <div className="text-sm text-theme-text-muted font-mono mt-1">
              <CopyableProcessId processId={processId} className="text-sm" />
            </div>
          </div>
          <div className="flex gap-3">
            {(() => {
              const processTimeRange = computeProcessTimeRange(
                screenLoadTime,
                process.start_time,
                process.last_update_time
              )
              const logHref = `/process_log?process_id=${processId}&from=${encodeURIComponent(processTimeRange.from)}&to=${encodeURIComponent(processTimeRange.to)}`
              return (
                <>
                  <AppLink
                    href={logHref}
                    className="flex items-center gap-2 px-4 py-2 bg-theme-border text-theme-text-primary rounded-md hover:bg-theme-border-hover transition-colors text-sm"
                  >
                    <FileText className="w-4 h-4" />
                    View Log
                  </AppLink>
                  <AppLink
                    href={`/process_metrics?process_id=${processId}&from=${encodeURIComponent(processTimeRange.from)}&to=${encodeURIComponent(processTimeRange.to)}`}
                    className="flex items-center gap-2 px-4 py-2 bg-theme-border text-theme-text-primary rounded-md hover:bg-theme-border-hover transition-colors text-sm"
                  >
                    <BarChart2 className="w-4 h-4" />
                    View Metrics
                  </AppLink>
                  <AppLink
                    href={`/performance_analysis?process_id=${processId}&from=${encodeURIComponent(processTimeRange.from)}&to=${encodeURIComponent(processTimeRange.to)}`}
                    className="flex items-center gap-2 px-4 py-2 bg-accent-link text-white rounded-md hover:bg-accent-link-hover transition-colors text-sm"
                  >
                    <Gauge className="w-4 h-4" />
                    Performance
                  </AppLink>
                </>
              )
            })()}
          </div>
        </div>

        {/* Info Cards Grid */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-5 mb-8">
          {/* Process Information */}
          <div className="bg-app-panel border border-theme-border rounded-lg p-5">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-theme-text-muted mb-4">
              Process Information
            </h3>
            <div className="space-y-0">
              <InfoRow label="Executable" value={String(process.exe ?? '')} />
              <InfoRow label="Process ID" value={processId} mono />
            </div>
          </div>

          {/* Environment */}
          <div className="bg-app-panel border border-theme-border rounded-lg p-5">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-theme-text-muted mb-4">
              Environment
            </h3>
            <div className="space-y-0">
              <InfoRow label="Computer" value={String(process.computer ?? '')} />
              <InfoRow label="Username" value={String(process.username ?? '')} />
              <InfoRow label="Distro" value={String(process.distro ?? '')} />
              <InfoRow label="CPU Brand" value={String(process.cpu_brand ?? '')} />
            </div>
          </div>

          {/* Timing */}
          <div className="bg-app-panel border border-theme-border rounded-lg p-5">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-theme-text-muted mb-4">
              Timing
            </h3>
            <div className="space-y-0">
              <InfoRow label="Start Time" value={formatLocalTime(process.start_time)} mono />
              <InfoRow label="Last Activity" value={formatLocalTime(process.last_update_time)} mono />
              <InfoRow
                label="Duration"
                value={formatDuration(process.start_time, process.last_update_time)}
              />
            </div>
          </div>

          {/* Statistics */}
          <div className="bg-app-panel border border-theme-border rounded-lg p-5">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-theme-text-muted mb-4">
              Statistics
            </h3>
            {statsError ? (
              <div className="text-sm text-accent-error">
                Failed to load statistics: {statsError}
              </div>
            ) : (
              <div className="space-y-0">
                <InfoRow
                  label="Log Entries"
                  value={statistics ? Number(statistics.log_entries ?? 0).toLocaleString() : '—'}
                />
                <InfoRow
                  label="Measures"
                  value={statistics ? Number(statistics.measures ?? 0).toLocaleString() : '—'}
                />
                <InfoRow
                  label="Trace Events"
                  value={statistics ? Number(statistics.trace_events ?? 0).toLocaleString() : '—'}
                />
                <InfoRow
                  label="Thread Count"
                  value={statistics ? Number(statistics.thread_count ?? 0).toLocaleString() : '—'}
                />
              </div>
            )}
          </div>

          {/* Properties */}
          <div className="bg-app-panel border border-theme-border rounded-lg p-5">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-theme-text-muted mb-4">
              Properties
            </h3>
            {propertiesError ? (
              <div className="text-sm text-accent-error">
                Failed to load properties: {propertiesError}
              </div>
            ) : properties === null ? (
              <div className="text-sm text-theme-text-muted">Loading...</div>
            ) : Object.keys(properties).length === 0 ? (
              <div className="text-sm text-theme-text-muted">No properties</div>
            ) : (
              <div className="space-y-0">
                {Object.entries(properties).map(([key, value]) => (
                  <InfoRow key={key} label={key} value={String(value)} />
                ))}
              </div>
            )}
          </div>
        </div>
      </div>
    </PageLayout>
  )
}

function InfoRow({
  label,
  value,
  mono = false,
}: {
  label: string
  value: string
  mono?: boolean
}) {
  return (
    <div className="flex justify-between py-2 border-b border-theme-border last:border-b-0">
      <span className="text-theme-text-muted text-sm">{label}</span>
      <span
        className={`text-sm text-right max-w-[60%] break-all ${
          mono ? 'font-mono' : ''
        } text-theme-text-primary`}
      >
        {value}
      </span>
    </div>
  )
}

export default function ProcessPage() {
  return (
    <AuthGuard>
      <Suspense
        fallback={
          <PageLayout>
            <div className="p-6">
              <div className="flex items-center justify-center h-64">
                <div className="animate-spin rounded-full h-8 w-8 border-2 border-accent-link border-t-transparent" />
              </div>
            </div>
          </PageLayout>
        }
      >
        <ProcessPageContent />
      </Suspense>
    </AuthGuard>
  )
}
