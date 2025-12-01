'use client'

import { Suspense, useState, useCallback, useMemo } from 'react'
import { useSearchParams } from 'next/navigation'
import { useQuery, useMutation } from '@tanstack/react-query'
import Link from 'next/link'
import { ArrowLeft, AlertCircle } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { fetchProcesses, fetchProcessLogEntries, executeSqlQuery } from '@/lib/api'
import { useTimeRange } from '@/hooks/useTimeRange'
import { LogEntry, SqlQueryResponse } from '@/types'

const DEFAULT_SQL = `SELECT time, level, target, msg
FROM log_entries
WHERE process_id = '$process_id'
  AND level <= $max_level
ORDER BY time DESC
LIMIT $limit`

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

// Convert SQL query results to LogEntry format
function sqlResultToLogEntries(result: SqlQueryResponse): LogEntry[] {
  const colIndex = (name: string) => result.columns.indexOf(name)
  const timeIdx = colIndex('time')
  const levelIdx = colIndex('level')
  const targetIdx = colIndex('target')
  const msgIdx = colIndex('msg')

  return result.rows.map((row) => {
    const levelValue = row[levelIdx]
    // Handle level as either number or string
    let levelStr: string
    if (typeof levelValue === 'number') {
      levelStr = LEVEL_NAMES[levelValue] || 'UNKNOWN'
    } else {
      levelStr = String(levelValue)
    }

    return {
      time: String(row[timeIdx] ?? ''),
      level: levelStr,
      target: String(row[targetIdx] ?? ''),
      msg: String(row[msgIdx] ?? ''),
    }
  })
}

function ProcessLogContent() {
  const searchParams = useSearchParams()
  const processId = searchParams.get('process_id')
  const { parsed: timeRange, apiTimeRange } = useTimeRange()

  const [logLevel, setLogLevel] = useState<string>('all')
  const [logLimit, setLogLimit] = useState<number>(100)
  const [queryError, setQueryError] = useState<string | null>(null)
  const [customSqlResults, setCustomSqlResults] = useState<LogEntry[] | null>(null)
  const [isUsingCustomQuery, setIsUsingCustomQuery] = useState(false)

  const { data: processes = [] } = useQuery({
    queryKey: ['processes'],
    queryFn: fetchProcesses,
  })

  const process = processes.find((p) => p.process_id === processId)

  const {
    data: logEntries = [],
    isLoading: logsLoading,
    refetch: refetchLogs,
  } = useQuery({
    queryKey: ['logs', processId, logLevel, logLimit],
    queryFn: () => fetchProcessLogEntries(processId!, logLevel, logLimit),
    enabled: !!processId,
    staleTime: 0,
  })

  const sqlMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      setQueryError(null)
      setCustomSqlResults(sqlResultToLogEntries(data))
      setIsUsingCustomQuery(true)
    },
    onError: (err: Error) => {
      setQueryError(err.message)
      setCustomSqlResults(null)
    },
  })

  const handleRunQuery = useCallback(
    (sql: string) => {
      setQueryError(null)
      const params: Record<string, string> = {
        process_id: processId || '',
        max_level: String(LOG_LEVELS[logLevel] || 6),
        limit: String(logLimit),
      }
      sqlMutation.mutate({
        sql,
        params,
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    },
    [sqlMutation, processId, logLevel, logLimit, apiTimeRange]
  )

  const handleResetQuery = useCallback(() => {
    setQueryError(null)
    setCustomSqlResults(null)
    setIsUsingCustomQuery(false)
    refetchLogs()
  }, [refetchLogs])

  // Use custom SQL results if available, otherwise use default query results
  const displayedLogEntries = isUsingCustomQuery && customSqlResults ? customSqlResults : logEntries

  const currentValues = useMemo(
    () => ({
      process_id: processId || '',
      max_level: String(LOG_LEVELS[logLevel] || 6),
      limit: String(logLimit),
    }),
    [processId, logLevel, logLimit]
  )

  const getLevelColor = (level: string) => {
    switch (level) {
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

  const formatTimestamp = (timestamp: string) => {
    return timestamp
  }

  const sqlPanel = processId ? (
    <QueryEditor
      defaultSql={DEFAULT_SQL}
      variables={VARIABLES}
      currentValues={currentValues}
      timeRangeLabel={timeRange.label}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      isLoading={logsLoading || sqlMutation.isPending}
      error={queryError}
    />
  ) : undefined

  const handleRefresh = useCallback(() => {
    if (isUsingCustomQuery) {
      setCustomSqlResults(null)
      setIsUsingCustomQuery(false)
    }
    refetchLogs()
  }, [isUsingCustomQuery, refetchLogs])

  if (!processId) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-[#1a1f26] border border-[#2f3540] rounded-lg">
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
          {process?.exe || 'Process'}
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
            className="px-3 py-2 bg-[#1a1f26] border border-[#2f3540] rounded-md text-gray-200 text-sm focus:outline-none focus:border-blue-500"
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
              className="px-3 py-2 bg-[#1a1f26] border border-[#2f3540] rounded-md text-gray-200 text-sm focus:outline-none focus:border-blue-500"
            >
              <option value={50}>50</option>
              <option value={100}>100</option>
              <option value={200}>200</option>
              <option value={500}>500</option>
              <option value={1000}>1000</option>
            </select>
          </div>

          <span className="ml-auto text-xs text-gray-500 self-center">
            {logsLoading || sqlMutation.isPending
              ? 'Loading...'
              : `Showing ${displayedLogEntries.length} entries${isUsingCustomQuery ? ' (custom query)' : ''}`}
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
        <div className="flex-1 overflow-auto bg-[#0d1117] border border-[#2f3540] rounded-lg font-mono text-xs">
          {logsLoading || sqlMutation.isPending ? (
            <div className="flex items-center justify-center h-full">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-5 w-5 border-2 border-blue-500 border-t-transparent" />
                <span className="text-gray-400">
                  {sqlMutation.isPending ? 'Executing query...' : 'Loading logs...'}
                </span>
              </div>
            </div>
          ) : displayedLogEntries.length === 0 ? (
            <div className="flex items-center justify-center h-full">
              <span className="text-gray-500">No log entries found</span>
            </div>
          ) : (
            <div>
              {displayedLogEntries.map((log, index) => (
                <div
                  key={index}
                  className="flex px-3 py-1 border-b border-[#1a1f26] hover:bg-[#161b22] transition-colors"
                >
                  <span className="text-gray-500 mr-4 whitespace-nowrap">
                    {formatTimestamp(log.time)}
                  </span>
                  <span className={`w-12 mr-3 font-semibold ${getLevelColor(log.level)}`}>
                    {log.level}
                  </span>
                  <span
                    className="text-purple-400 mr-3 max-w-[200px] truncate"
                    title={log.target}
                  >
                    {log.target}
                  </span>
                  <span className="text-gray-200 flex-1 break-words">{log.msg}</span>
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
