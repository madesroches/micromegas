import { useCallback, useEffect, useRef } from 'react'
import { Table } from 'apache-arrow'
import { useChangeEffect } from '@/hooks/useChangeEffect'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { getTimeRangeForApi } from '@/lib/time-range'

export interface ScreenQueryParams {
  /** Initial SQL from config */
  initialSql: string
  /** Raw time range (relative strings like "now-1h") resolved fresh at execution time */
  rawTimeRange: { from: string; to: string }
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
  rawTimeRange,
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

      const timeRange = getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to)
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
    [rawTimeRange, params, transformSql, dataSource]
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
  const prevTimeRangeRef = useRef(rawTimeRange)
  useEffect(() => {
    if (
      prevTimeRangeRef.current.from !== rawTimeRange.from ||
      prevTimeRangeRef.current.to !== rawTimeRange.to
    ) {
      prevTimeRangeRef.current = rawTimeRange
      executeQuery(currentSqlRef.current)
    }
  }, [rawTimeRange, executeQuery])

  // Re-execute on data source change
  useChangeEffect(dataSource, () => executeQuery(currentSqlRef.current))

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
