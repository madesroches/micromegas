import { useState, useCallback, useEffect, useRef } from 'react'
import { ChevronUp, ChevronDown } from 'lucide-react'
import { registerRenderer, ScreenRendererProps } from './index'
import { useTimeRangeSync } from './useTimeRangeSync'
import { useSqlHandlers } from './useSqlHandlers'
import { LoadingState, EmptyState, SaveFooter, RendererLayout } from './shared'
import { QueryEditor } from '@/components/QueryEditor'
import { AppLink } from '@/components/AppLink'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { formatTimestamp, formatDuration } from '@/lib/time-range'
import { timestampToDate } from '@/lib/arrow-utils'
import { useStreamQuery } from '@/hooks/useStreamQuery'

// Variables available for process list queries
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
]

// Sorting types
type ProcessSortField = 'exe' | 'start_time' | 'last_update_time' | 'runtime' | 'username' | 'computer'
type SortDirection = 'asc' | 'desc'

// Map UI field names to SQL expressions
const SORT_FIELD_TO_SQL: Record<ProcessSortField, string> = {
  exe: 'exe',
  start_time: 'start_time',
  last_update_time: 'last_update_time',
  runtime: '(last_update_time - start_time)',
  username: 'username',
  computer: 'computer',
}

interface ProcessListConfig {
  sql: string
  timeRangeFrom?: string
  timeRangeTo?: string
  [key: string]: unknown
}

/**
 * Transforms SQL to apply the requested sort order.
 * Replaces existing ORDER BY clause or appends before LIMIT.
 */
function applySortToSql(sql: string, field: ProcessSortField, direction: SortDirection): string {
  const sqlExpr = SORT_FIELD_TO_SQL[field]
  const orderClause = `ORDER BY ${sqlExpr} ${direction.toUpperCase()}`

  // Check if there's an existing ORDER BY clause (case insensitive)
  const orderByRegex = /ORDER\s+BY\s+[^)]+?(?=\s+LIMIT|\s*$)/i
  if (orderByRegex.test(sql)) {
    return sql.replace(orderByRegex, orderClause)
  }

  // No ORDER BY - insert before LIMIT if present, otherwise append
  const limitRegex = /(\s+LIMIT\s+)/i
  if (limitRegex.test(sql)) {
    return sql.replace(limitRegex, `\n${orderClause}$1`)
  }

  // No LIMIT either - just append
  return `${sql.trimEnd()}\n${orderClause}`
}

export function ProcessListRenderer({
  config,
  onConfigChange,
  savedConfig,
  onUnsavedChange,
  timeRange,
  rawTimeRange,
  timeRangeLabel,
  currentValues,
  onSave,
  isSaving,
  hasUnsavedChanges,
  onSaveAs,
  saveError,
  refreshTrigger,
}: ScreenRendererProps) {
  const processListConfig = config as unknown as ProcessListConfig
  const savedProcessListConfig = savedConfig as unknown as ProcessListConfig | null

  // Sorting state (UI-only, not persisted)
  const [sortField, setSortField] = useState<ProcessSortField>('last_update_time')
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc')

  // Query execution - using useStreamQuery directly to control re-execution on sort change
  const streamQuery = useStreamQuery()
  const queryError = streamQuery.error?.message ?? null

  // Track current SQL for re-execution
  const currentSqlRef = useRef<string>(processListConfig.sql)
  const executeRef = useRef(streamQuery.execute)
  executeRef.current = streamQuery.execute

  // Execute query with current sort applied
  const executeWithSort = useCallback(
    (sql: string) => {
      currentSqlRef.current = sql
      const sortedSql = applySortToSql(sql, sortField, sortDirection)

      executeRef.current({
        sql: sortedSql,
        params: {
          begin: timeRange.begin,
          end: timeRange.end,
        },
        begin: timeRange.begin,
        end: timeRange.end,
      })
    },
    [timeRange, sortField, sortDirection]
  )

  // Initial query execution
  const hasExecutedRef = useRef(false)
  useEffect(() => {
    if (!hasExecutedRef.current) {
      hasExecutedRef.current = true
      executeWithSort(processListConfig.sql)
    }
  }, []) // eslint-disable-line react-hooks/exhaustive-deps

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
      executeWithSort(currentSqlRef.current)
    }
  }, [timeRange, executeWithSort])

  // Re-execute on refresh trigger
  const prevRefreshTriggerRef = useRef(refreshTrigger)
  useEffect(() => {
    if (prevRefreshTriggerRef.current !== refreshTrigger) {
      prevRefreshTriggerRef.current = refreshTrigger
      executeWithSort(currentSqlRef.current)
    }
  }, [refreshTrigger, executeWithSort])

  // Re-execute when sort changes
  const prevSortRef = useRef<{ field: ProcessSortField; direction: SortDirection } | null>(null)
  useEffect(() => {
    if (prevSortRef.current === null) {
      prevSortRef.current = { field: sortField, direction: sortDirection }
      return
    }
    if (prevSortRef.current.field !== sortField || prevSortRef.current.direction !== sortDirection) {
      prevSortRef.current = { field: sortField, direction: sortDirection }
      executeWithSort(currentSqlRef.current)
    }
  }, [sortField, sortDirection, executeWithSort])

  // Sync time range changes to config
  useTimeRangeSync({
    rawTimeRange,
    savedConfig: savedProcessListConfig,
    config: processListConfig,
    onUnsavedChange,
    onConfigChange,
  })

  // SQL editor handlers
  const { handleRunQuery, handleResetQuery, handleSqlChange } = useSqlHandlers({
    config: processListConfig,
    savedConfig: savedProcessListConfig,
    onConfigChange,
    onUnsavedChange,
    execute: (sql: string) => executeWithSort(sql),
  })

  const handleSort = useCallback(
    (field: ProcessSortField) => {
      if (sortField === field) {
        setSortDirection(sortDirection === 'asc' ? 'desc' : 'asc')
      } else {
        setSortField(field)
        setSortDirection('desc')
      }
    },
    [sortField, sortDirection]
  )

  // Sort header component
  const SortHeader = ({
    field,
    children,
    className = '',
  }: {
    field: ProcessSortField
    children: React.ReactNode
    className?: string
  }) => (
    <th
      onClick={() => handleSort(field)}
      className={`px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider cursor-pointer select-none transition-colors ${
        sortField === field
          ? 'text-theme-text-primary bg-app-card'
          : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-app-card'
      } ${className}`}
    >
      <div className="flex items-center gap-1">
        {children}
        <span className={sortField === field ? 'text-accent-link' : 'opacity-30'}>
          {sortField === field && sortDirection === 'asc' ? (
            <ChevronUp className="w-3 h-3" />
          ) : (
            <ChevronDown className="w-3 h-3" />
          )}
        </span>
      </div>
    </th>
  )

  // Query editor panel
  const sqlPanel = (
    <QueryEditor
      defaultSql={savedProcessListConfig ? savedProcessListConfig.sql : processListConfig.sql}
      variables={VARIABLES}
      currentValues={currentValues}
      timeRangeLabel={timeRangeLabel}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      onChange={handleSqlChange}
      isLoading={streamQuery.isStreaming}
      error={queryError}
      footer={
        <SaveFooter
          onSave={onSave}
          onSaveAs={onSaveAs}
          isSaving={isSaving}
          hasUnsavedChanges={hasUnsavedChanges}
          saveError={saveError}
        />
      }
    />
  )

  const handleRetry = useCallback(() => {
    executeWithSort(currentSqlRef.current)
  }, [executeWithSort])

  // Render content
  const renderContent = () => {
    const table = streamQuery.getTable()

    if (streamQuery.isStreaming && !table) {
      return <LoadingState message="Loading data..." />
    }

    if (!table || table.numRows === 0) {
      return <EmptyState message="No processes available." />
    }

    return (
      <div className="flex-1 overflow-auto bg-app-panel border border-theme-border rounded-lg">
        <table className="w-full">
          <thead className="sticky top-0">
            <tr className="bg-app-card border-b border-theme-border">
              <SortHeader field="exe">Process</SortHeader>
              <th className="hidden sm:table-cell px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider text-theme-text-muted">
                Process ID
              </th>
              <SortHeader field="start_time">Start Time</SortHeader>
              <SortHeader field="last_update_time" className="hidden lg:table-cell">
                Last Update
              </SortHeader>
              <SortHeader field="runtime" className="hidden lg:table-cell">
                Runtime
              </SortHeader>
              <SortHeader field="username" className="hidden md:table-cell">
                Username
              </SortHeader>
              <SortHeader field="computer" className="hidden md:table-cell">
                Computer
              </SortHeader>
            </tr>
          </thead>
          <tbody>
            {Array.from({ length: table.numRows }, (_, i) => {
              const row = table.get(i)
              if (!row) return null
              const processId = String(row.process_id ?? '')
              const exe = String(row.exe ?? '')
              const startTime = row.start_time
              const lastUpdateTime = row.last_update_time
              const username = String(row.username ?? '')
              const computer = String(row.computer ?? '')
              const startDate = timestampToDate(startTime)
              const endDate = timestampToDate(lastUpdateTime)
              const fromParam = startDate?.toISOString() ?? ''
              const toParam = endDate?.toISOString() ?? ''
              return (
                <tr
                  key={processId || i}
                  className="border-b border-theme-border hover:bg-app-card transition-colors"
                >
                  <td className="px-4 py-3">
                    <AppLink
                      href={`/process?process_id=${processId}&from=${encodeURIComponent(fromParam)}&to=${encodeURIComponent(toParam)}`}
                      className="text-accent-link hover:underline"
                    >
                      {exe}
                    </AppLink>
                  </td>
                  <td className="hidden sm:table-cell px-4 py-3">
                    <CopyableProcessId
                      processId={processId}
                      truncate={true}
                      className="text-sm font-mono text-theme-text-secondary"
                    />
                  </td>
                  <td className="px-4 py-3 font-mono text-sm text-theme-text-primary">
                    {formatTimestamp(startTime)}
                  </td>
                  <td className="hidden lg:table-cell px-4 py-3 font-mono text-sm text-theme-text-primary">
                    {formatTimestamp(lastUpdateTime)}
                  </td>
                  <td className="hidden lg:table-cell px-4 py-3 font-mono text-sm text-theme-text-secondary">
                    {formatDuration(startTime, lastUpdateTime)}
                  </td>
                  <td className="hidden md:table-cell px-4 py-3 text-theme-text-primary">
                    {username}
                  </td>
                  <td className="hidden md:table-cell px-4 py-3 text-theme-text-primary">
                    {computer}
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>
    )
  }

  return (
    <RendererLayout
      error={queryError}
      isRetryable={streamQuery.error?.retryable}
      onRetry={handleRetry}
      sqlPanel={sqlPanel}
    >
      {renderContent()}
    </RendererLayout>
  )
}

// Register this renderer
registerRenderer('process_list', ProcessListRenderer)
