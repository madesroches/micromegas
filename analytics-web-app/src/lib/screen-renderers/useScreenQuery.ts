import { useCallback, useEffect, useRef } from 'react'
import { Table } from 'apache-arrow'
import { useStreamQuery } from '@/hooks/useStreamQuery'

export interface ScreenQueryParams {
  /** Initial SQL from config */
  initialSql: string
  /** Time range for query */
  timeRange: { begin: string; end: string }
  /** Increment to trigger refresh */
  refreshTrigger: number
  /** Additional query params (type-specific) */
  params?: Record<string, string>
  /** Transform SQL before execution (e.g., variable expansion) */
  transformSql?: (sql: string) => string
  /** Data source name for query routing */
  dataSource?: string
}

export interface ScreenQueryResult {
  /** Arrow table from query result */
  table: Table | null
  /** Whether query is currently executing */
  isLoading: boolean
  /** Whether query has completed at least once */
  isComplete: boolean
  /** Error message if query failed */
  error: string | null
  /** Whether error is retryable */
  isRetryable: boolean
  /** Execute query with given SQL */
  execute: (sql: string) => void
  /** Retry last query */
  retry: () => void
  /** Current SQL (may differ from initialSql if user edited) */
  currentSql: string
}

/**
 * Hook for managing screen query execution.
 *
 * Handles:
 * - Initial query execution
 * - Re-execution on time range change
 * - Re-execution on refresh trigger
 * - SQL tracking across edits
 *
 * This is an optional helper - renderers can use useStreamQuery directly
 * if they need more control.
 */
export function useScreenQuery({
  initialSql,
  timeRange,
  refreshTrigger,
  params = {},
  transformSql,
  dataSource,
}: ScreenQueryParams): ScreenQueryResult {
  const streamQuery = useStreamQuery()

  // Track current SQL (may be edited by user)
  const currentSqlRef = useRef<string>(initialSql)

  // Stable reference to execute function
  const executeRef = useRef(streamQuery.execute)
  executeRef.current = streamQuery.execute

  // Execute query
  const executeQuery = useCallback(
    (sql: string) => {
      currentSqlRef.current = sql
      const finalSql = transformSql ? transformSql(sql) : sql

      executeRef.current({
        sql: finalSql,
        params: {
          begin: timeRange.begin,
          end: timeRange.end,
          ...params,
        },
        begin: timeRange.begin,
        end: timeRange.end,
        dataSource,
      })
    },
    [timeRange, params, transformSql, dataSource]
  )

  // Initial query execution (wait for dataSource to resolve)
  const hasExecutedRef = useRef(false)
  useEffect(() => {
    if (!hasExecutedRef.current && dataSource) {
      hasExecutedRef.current = true
      executeQuery(initialSql)
    }
  }, [dataSource]) // eslint-disable-line react-hooks/exhaustive-deps

  // Re-execute on time range change
  const prevTimeRangeRef = useRef<{ begin: string; end: string } | null>(null)
  useEffect(() => {
    if (prevTimeRangeRef.current === null) {
      prevTimeRangeRef.current = { begin: timeRange.begin, end: timeRange.end }
      return
    }
    if (
      prevTimeRangeRef.current.begin !== timeRange.begin ||
      prevTimeRangeRef.current.end !== timeRange.end
    ) {
      prevTimeRangeRef.current = { begin: timeRange.begin, end: timeRange.end }
      executeQuery(currentSqlRef.current)
    }
  }, [timeRange, executeQuery])

  // Re-execute on data source change
  const prevDataSourceRef = useRef<string | null>(null)
  useEffect(() => {
    if (prevDataSourceRef.current === null) {
      prevDataSourceRef.current = dataSource || ''
      return
    }
    if (prevDataSourceRef.current !== (dataSource || '')) {
      prevDataSourceRef.current = dataSource || ''
      executeQuery(currentSqlRef.current)
    }
  }, [dataSource, executeQuery])

  // Re-execute on refresh trigger
  const prevRefreshTriggerRef = useRef(refreshTrigger)
  useEffect(() => {
    if (prevRefreshTriggerRef.current !== refreshTrigger) {
      prevRefreshTriggerRef.current = refreshTrigger
      executeQuery(currentSqlRef.current)
    }
  }, [refreshTrigger, executeQuery])

  // Retry handler
  const retry = useCallback(() => {
    executeQuery(currentSqlRef.current)
  }, [executeQuery])

  return {
    table: streamQuery.getTable(),
    isLoading: streamQuery.isStreaming,
    isComplete: streamQuery.isComplete,
    error: streamQuery.error?.message ?? null,
    isRetryable: streamQuery.error?.retryable ?? false,
    execute: executeQuery,
    retry,
    currentSql: currentSqlRef.current,
  }
}
