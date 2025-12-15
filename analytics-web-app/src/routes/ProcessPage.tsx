import { Suspense, useState, useEffect, useCallback, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { useMutation } from '@tanstack/react-query'
import { AppLink } from '@/components/AppLink'
import { ArrowLeft, FileText, Activity, AlertCircle, BarChart2, Gauge } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { executeSqlQuery, toRowObjects } from '@/lib/api'
import { useTimeRange } from '@/hooks/useTimeRange'
import { formatDuration, formatDateTimeLocal } from '@/lib/time-range'
import { SqlRow } from '@/types'

const PROCESS_SQL = `SELECT process_id, exe, start_time, last_update_time, computer, username, cpu_brand, distro
FROM processes
WHERE process_id = '$process_id'
LIMIT 1`

function formatLocalTime(timestamp: unknown): string {
  if (!timestamp) return '—'
  const date = new Date(String(timestamp))
  if (isNaN(date.getTime())) return String(timestamp)
  return formatDateTimeLocal(date)
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

function ProcessPageContent() {
  const [searchParams] = useSearchParams()
  const processId = searchParams.get('id')
  const { apiTimeRange, timeRange } = useTimeRange()

  const [process, setProcess] = useState<SqlRow | null>(null)
  const [statistics, setStatistics] = useState<SqlRow | null>(null)
  const [statsError, setStatsError] = useState<string | null>(null)
  const [properties, setProperties] = useState<Record<string, string> | null>(null)
  const [propertiesError, setPropertiesError] = useState<string | null>(null)
  const [isLoading, setIsLoading] = useState(true)

  const processMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      const rows = toRowObjects(data)
      if (rows.length > 0) {
        setProcess(rows[0])
      }
      setIsLoading(false)
    },
    onError: () => {
      setIsLoading(false)
    },
  })

  const statsMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      setStatsError(null)
      const rows = toRowObjects(data)
      if (rows.length > 0) {
        setStatistics(rows[0])
      }
    },
    onError: (err: Error) => {
      setStatsError(err.message)
    },
  })

  const propertiesMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      setPropertiesError(null)
      const rows = toRowObjects(data)
      if (rows.length > 0 && rows[0].properties) {
        try {
          const parsed = JSON.parse(String(rows[0].properties))
          setProperties(parsed)
        } catch {
          setPropertiesError('Failed to parse properties')
        }
      } else {
        setProperties({})
      }
    },
    onError: (err: Error) => {
      setPropertiesError(err.message)
    },
  })

  // Use refs to avoid including mutations in callback deps
  const processMutateRef = useRef(processMutation.mutate)
  processMutateRef.current = processMutation.mutate
  const statsMutateRef = useRef(statsMutation.mutate)
  statsMutateRef.current = statsMutation.mutate
  const propertiesMutateRef = useRef(propertiesMutation.mutate)
  propertiesMutateRef.current = propertiesMutation.mutate

  const loadData = useCallback(() => {
    if (!processId) return
    setIsLoading(true)
    processMutateRef.current({
      sql: PROCESS_SQL,
      params: { process_id: processId },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    })
    statsMutateRef.current({
      sql: STATISTICS_SQL,
      params: { process_id: processId },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    })
    propertiesMutateRef.current({
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
            <AppLink
              href={`/process_log?process_id=${processId}&from=${encodeURIComponent(timeRange.from)}&to=${encodeURIComponent(timeRange.to)}`}
              className="flex items-center gap-2 px-4 py-2 bg-theme-border text-theme-text-primary rounded-md hover:bg-theme-border-hover transition-colors text-sm"
            >
              <FileText className="w-4 h-4" />
              View Log
            </AppLink>
            <AppLink
              href={`/process_metrics?process_id=${processId}&from=${encodeURIComponent(timeRange.from)}&to=${encodeURIComponent(timeRange.to)}`}
              className="flex items-center gap-2 px-4 py-2 bg-theme-border text-theme-text-primary rounded-md hover:bg-theme-border-hover transition-colors text-sm"
            >
              <BarChart2 className="w-4 h-4" />
              View Metrics
            </AppLink>
            <AppLink
              href={`/performance_analysis?process_id=${processId}&from=${encodeURIComponent(timeRange.from)}&to=${encodeURIComponent(timeRange.to)}`}
              className="flex items-center gap-2 px-4 py-2 bg-theme-border text-theme-text-primary rounded-md hover:bg-theme-border-hover transition-colors text-sm"
            >
              <Gauge className="w-4 h-4" />
              Performance
            </AppLink>
            <AppLink
              href={`/process_trace?process_id=${processId}&from=${encodeURIComponent(timeRange.from)}&to=${encodeURIComponent(timeRange.to)}`}
              className="flex items-center gap-2 px-4 py-2 bg-accent-link text-white rounded-md hover:bg-accent-link-hover transition-colors text-sm"
            >
              <Activity className="w-4 h-4" />
              Generate Trace
            </AppLink>
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
