'use client'

import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useSearchParams } from 'next/navigation'
import { useMutation } from '@tanstack/react-query'
import Link from 'next/link'
import { ArrowLeft, AlertCircle } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { executeSqlQuery, toRowObjects } from '@/lib/api'
import { useTimeRange } from '@/hooks/useTimeRange'
import { SqlRow } from '@/types'

const DEFAULT_SQL = `SELECT time, level, target, msg
FROM log_entries
WHERE process_id = '$process_id'
  AND level <= $max_level
ORDER BY time DESC
LIMIT $limit`

const PROCESS_SQL = `SELECT exe FROM processes WHERE process_id = '$process_id' LIMIT 1`

const VARIABLES = [
  { name: 'process_id', description: 'Current process ID' },
  { name: 'max_level', description: 'Max log level filter (1-6)' },
  { name: 'limit', description: 'Row limit' },
]

const LOG_LEVELS: Record<string, number> = {
  all: 6,
  trace: 6,
  debug: 5,
  info: 4,
  warn: 3,
  error: 2,
  fatal: 1,
}

const LEVEL_NAMES: Record<number, string> = {
  1: 'FATAL',
  2: 'ERROR',
  3: 'WARN',
  4: 'INFO',
  5: 'DEBUG',
  6: 'TRACE',
}

function ProcessLogContent() {
  const searchParams = useSearchParams()
  const processId = searchParams.get('process_id')
  const { parsed: timeRange, apiTimeRange } = useTimeRange()

  const [logLevel, setLogLevel] = useState<string>('all')
  const [logLimit, setLogLimit] = useState<number>(100)
  const [queryError, setQueryError] = useState<string | null>(null)
  const [rows, setRows] = useState<SqlRow[]>([])
  const [processExe, setProcessExe] = useState<string | null>(null)
  const [hasLoaded, setHasLoaded] = useState(false)

  const sqlMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      setQueryError(null)
      const resultRows = toRowObjects(data)
      // Normalize level values
      setRows(resultRows.map(row => ({
        ...row,
        level: typeof row.level === 'number' ? (LEVEL_NAMES[row.level] || 'UNKNOWN') : row.level
      })))
      setHasLoaded(true)
    },
    onError: (err: Error) => {
      setQueryError(err.message)
    },
  })

  const processMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      const resultRows = toRowObjects(data)
      if (resultRows.length > 0) {
        setProcessExe(String(resultRows[0].exe ?? ''))
      }
    },
  })

  // Use refs to avoid including mutations in callback deps
  const sqlMutateRef = useRef(sqlMutation.mutate)
  sqlMutateRef.current = sqlMutation.mutate
  const processMutateRef = useRef(processMutation.mutate)
  processMutateRef.current = processMutation.mutate

  const loadData = useCallback(
    (sql: string = DEFAULT_SQL) => {
      if (!processId) return
      setQueryError(null)
      const params: Record<string, string> = {
        process_id: processId,
        max_level: String(LOG_LEVELS[logLevel] || 6),
        limit: String(logLimit),
      }
      sqlMutateRef.current({
        sql,
        params,
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    },
    [processId, logLevel, logLimit, apiTimeRange]
  )

  // Load process info once
  const hasLoadedProcessRef = useRef(false)
  useEffect(() => {
    if (processId && !hasLoadedProcessRef.current) {
      hasLoadedProcessRef.current = true
      processMutateRef.current({
        sql: PROCESS_SQL,
        params: { process_id: processId },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    }
  }, [processId, apiTimeRange])

  // Initial load
  const hasInitialLoadRef = useRef(false)
  useEffect(() => {
    if (processId && !hasInitialLoadRef.current) {
      hasInitialLoadRef.current = true
      loadData()
    }
  }, [processId, loadData])

  // Reload when filters change (only after initial load)
  const prevFiltersRef = useRef<{ logLevel: string; logLimit: number } | null>(null)
  useEffect(() => {
    // Skip if we haven't done initial load yet
    if (!hasLoaded) return

    // Initialize ref on first run after initial load
    if (prevFiltersRef.current === null) {
      prevFiltersRef.current = { logLevel, logLimit }
      return
    }

    // Check if filters actually changed
    if (prevFiltersRef.current.logLevel !== logLevel || prevFiltersRef.current.logLimit !== logLimit) {
      prevFiltersRef.current = { logLevel, logLimit }
      loadData()
    }
  }, [logLevel, logLimit, hasLoaded, loadData])

  // Reload when time range changes (only after initial load)
  const prevTimeRangeRef = useRef<{ begin: string; end: string } | null>(null)
  useEffect(() => {
    // Skip if we haven't done initial load yet
    if (!hasLoaded) return

    // Initialize ref on first run after initial load
    if (prevTimeRangeRef.current === null) {
      prevTimeRangeRef.current = { begin: apiTimeRange.begin, end: apiTimeRange.end }
      return
    }

    // Check if time range actually changed
    if (prevTimeRangeRef.current.begin !== apiTimeRange.begin || prevTimeRangeRef.current.end !== apiTimeRange.end) {
      prevTimeRangeRef.current = { begin: apiTimeRange.begin, end: apiTimeRange.end }
      loadData()
    }
  }, [apiTimeRange.begin, apiTimeRange.end, hasLoaded, loadData])

  const handleRunQuery = useCallback(
    (sql: string) => {
      loadData(sql)
    },
    [loadData]
  )

  const handleResetQuery = useCallback(() => {
    loadData(DEFAULT_SQL)
  }, [loadData])

  const currentValues = useMemo(
    () => ({
      process_id: processId || '',
      max_level: String(LOG_LEVELS[logLevel] || 6),
      limit: String(logLimit),
    }),
    [processId, logLevel, logLimit]
  )

  const getLevelColor = (level: unknown) => {
    const levelStr = String(level)
    switch (levelStr) {
      case 'FATAL':
        return 'text-red-600'
      case 'ERROR':
        return 'text-red-400'
      case 'WARN':
        return 'text-yellow-400'
      case 'INFO':
        return 'text-blue-400'
      case 'DEBUG':
        return 'text-gray-400'
      case 'TRACE':
        return 'text-gray-500'
      default:
        return 'text-gray-300'
    }
  }

  const sqlPanel = processId ? (
    <QueryEditor
      defaultSql={DEFAULT_SQL}
      variables={VARIABLES}
      currentValues={currentValues}
      timeRangeLabel={timeRange.label}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      isLoading={sqlMutation.isPending}
      error={queryError}
    />
  ) : undefined

  const handleRefresh = useCallback(() => {
    loadData()
  }, [loadData])

  if (!processId) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-app-panel border border-theme-border rounded-lg">
            <AlertCircle className="w-10 h-10 text-red-400 mb-3" />
            <p className="text-gray-400">No process ID provided</p>
            <Link href="/processes" className="text-blue-400 hover:underline mt-2">
              Back to Processes
            </Link>
          </div>
        </div>
      </PageLayout>
    )
  }

  return (
    <PageLayout onRefresh={handleRefresh} rightPanel={sqlPanel}>
      <div className="p-6 flex flex-col h-full">
        {/* Back Link */}
        <Link
          href={`/process?id=${processId}`}
          className="inline-flex items-center gap-1.5 text-blue-400 hover:underline text-sm mb-4"
        >
          <ArrowLeft className="w-3 h-3" />
          {processExe || 'Process'}
        </Link>

        {/* Page Header */}
        <div className="mb-5">
          <h1 className="text-2xl font-semibold text-gray-200">Process Log</h1>
          <div className="text-sm text-gray-500 font-mono mt-1">
            <CopyableProcessId processId={processId} className="text-sm" />
          </div>
        </div>

        {/* Filters */}
        <div className="flex gap-3 mb-4">
          <select
            value={logLevel}
            onChange={(e) => setLogLevel(e.target.value)}
            className="px-3 py-2 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-blue"
          >
            <option value="all">Max Level: TRACE (all)</option>
            <option value="debug">Max Level: DEBUG</option>
            <option value="info">Max Level: INFO</option>
            <option value="warn">Max Level: WARN</option>
            <option value="error">Max Level: ERROR</option>
            <option value="fatal">Max Level: FATAL</option>
          </select>

          <div className="flex items-center gap-2">
            <span className="text-gray-500 text-sm">Limit:</span>
            <select
              value={logLimit}
              onChange={(e) => setLogLimit(Number(e.target.value))}
              className="px-3 py-2 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-blue"
            >
              <option value={50}>50</option>
              <option value={100}>100</option>
              <option value={200}>200</option>
              <option value={500}>500</option>
              <option value={1000}>1000</option>
            </select>
          </div>

          <span className="ml-auto text-xs text-gray-500 self-center">
            {sqlMutation.isPending && rows.length === 0
              ? 'Loading...'
              : `Showing ${rows.length} entries`}
          </span>
        </div>

        {/* Query Error Banner */}
        {queryError && (
          <ErrorBanner
            title="Query execution failed"
            message={queryError}
            onDismiss={() => setQueryError(null)}
            onRetry={handleRefresh}
          />
        )}

        {/* Log Viewer */}
        <div className="flex-1 overflow-auto bg-app-bg border border-theme-border rounded-lg font-mono text-xs">
          {sqlMutation.isPending && !hasLoaded ? (
            <div className="flex items-center justify-center h-full">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-5 w-5 border-2 border-blue-500 border-t-transparent" />
                <span className="text-gray-400">Loading logs...</span>
              </div>
            </div>
          ) : rows.length === 0 ? (
            <div className="flex items-center justify-center h-full">
              <span className="text-gray-500">No log entries found</span>
            </div>
          ) : (
            <div>
              {rows.map((row, index) => (
                <div
                  key={index}
                  className="flex px-3 py-1 border-b border-app-panel hover:bg-app-panel/50 transition-colors"
                >
                  <span className="text-gray-500 mr-4 whitespace-nowrap">
                    {String(row.time ?? '')}
                  </span>
                  <span className={`w-12 mr-3 font-semibold ${getLevelColor(row.level)}`}>
                    {String(row.level ?? '')}
                  </span>
                  <span
                    className="text-purple-400 mr-3 max-w-[200px] truncate"
                    title={String(row.target ?? '')}
                  >
                    {String(row.target ?? '')}
                  </span>
                  <span className="text-gray-200 flex-1 break-words">{String(row.msg ?? '')}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </PageLayout>
  )
}

export default function ProcessLogPage() {
  return (
    <AuthGuard>
      <Suspense
        fallback={
          <PageLayout>
            <div className="p-6">
              <div className="flex items-center justify-center h-64">
                <div className="animate-spin rounded-full h-8 w-8 border-2 border-blue-500 border-t-transparent" />
              </div>
            </div>
          </PageLayout>
        }
      >
        <ProcessLogContent />
      </Suspense>
    </AuthGuard>
  )
}
