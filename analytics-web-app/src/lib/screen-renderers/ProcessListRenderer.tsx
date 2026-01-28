import { useState, useCallback, useEffect, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { ChevronUp, ChevronDown } from 'lucide-react'
import { DataType, Field } from 'apache-arrow'
import { registerRenderer, ScreenRendererProps } from './index'
import { useTimeRangeSync } from './useTimeRangeSync'
import { useSqlHandlers } from './useSqlHandlers'
import { LoadingState, EmptyState, SaveFooter, RendererLayout } from './shared'
import { QueryEditor } from '@/components/QueryEditor'
import { AppLink } from '@/components/AppLink'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { formatTimestamp } from '@/lib/time-range'
import { timestampToDate, isTimeType } from '@/lib/arrow-utils'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useDefaultSaveCleanup } from '@/lib/url-cleanup-utils'

// Variables available for process list queries
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
]

type SortDirection = 'asc' | 'desc'

// Column metadata for special handling
interface ColumnMeta {
  label: string
  sortable: boolean
  sqlExpr?: string // SQL expression for sorting if different from column name
}

// Known columns with special rendering/sorting behavior
const KNOWN_COLUMNS: Record<string, ColumnMeta> = {
  exe: { label: 'Process', sortable: true },
  process_id: { label: 'Process ID', sortable: false },
  start_time: { label: 'Start Time', sortable: true },
  last_update_time: { label: 'Last Update', sortable: true },
  username: { label: 'Username', sortable: true },
  computer: { label: 'Computer', sortable: true },
  distro: { label: 'Distro', sortable: true },
  cpu_brand: { label: 'CPU', sortable: true },
  parent_process_id: { label: 'Parent Process ID', sortable: false },
}

// Get column metadata - use known metadata or generate from column name
function getColumnMeta(columnName: string): ColumnMeta {
  if (KNOWN_COLUMNS[columnName]) {
    return KNOWN_COLUMNS[columnName]
  }
  // Generate label from column name (snake_case to Title Case)
  const label = columnName
    .split('_')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(' ')
  return { label, sortable: true }
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
function applySortToSql(sql: string, field: string, direction: SortDirection): string {
  const meta = getColumnMeta(field)
  if (!meta.sortable) {
    return sql
  }
  const sqlExpr = meta.sqlExpr ?? field
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
  setHasUnsavedChanges,
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
  const [, setSearchParams] = useSearchParams()
  const handleSave = useDefaultSaveCleanup(onSave, setSearchParams)

  // Sorting state (UI-only, not persisted)
  const [sortField, setSortField] = useState<string>('last_update_time')
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
  const prevSortRef = useRef<{ field: string; direction: SortDirection } | null>(null)
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
    setHasUnsavedChanges,
    onConfigChange,
  })

  // SQL editor handlers
  const { handleRunQuery, handleResetQuery, handleSqlChange } = useSqlHandlers({
    config: processListConfig,
    savedConfig: savedProcessListConfig,
    onConfigChange,
    setHasUnsavedChanges,
    execute: (sql: string) => executeWithSort(sql),
  })

  const handleSort = useCallback(
    (field: string) => {
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
    sortable,
    children,
    className = '',
  }: {
    field: string
    sortable: boolean
    children: React.ReactNode
    className?: string
  }) => {
    if (!sortable) {
      return (
        <th
          className={`px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider text-theme-text-muted ${className}`}
        >
          {children}
        </th>
      )
    }
    return (
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
  }

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
          onSave={handleSave}
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

  // Render a cell value based on column type and name
  const renderCell = useCallback(
    (
      columnName: string,
      dataType: DataType,
      row: Record<string, unknown>,
      processId: string,
      fromParam: string,
      toParam: string
    ) => {
      const value = row[columnName]

      // Special rendering for known columns
      if (columnName === 'exe') {
        // Use process times if available, otherwise fall back to screen's time range
        const linkFrom = fromParam || timeRange.begin
        const linkTo = toParam || timeRange.end
        return (
          <AppLink
            href={`/process?process_id=${processId}&from=${encodeURIComponent(linkFrom)}&to=${encodeURIComponent(linkTo)}`}
            className="text-accent-link hover:underline"
          >
            {String(value ?? '')}
          </AppLink>
        )
      }

      if (columnName === 'process_id' || columnName === 'parent_process_id') {
        const id = String(value ?? '')
        return (
          <CopyableProcessId
            processId={id}
            truncate={true}
            className="text-sm font-mono text-theme-text-secondary"
          />
        )
      }

      // Check if it's a timestamp type using Arrow type
      if (isTimeType(dataType)) {
        return (
          <span className="font-mono text-sm text-theme-text-primary">{formatTimestamp(value)}</span>
        )
      }

      // Default rendering
      return <span className="text-theme-text-primary">{String(value ?? '')}</span>
    },
    [timeRange.begin, timeRange.end]
  )

  // Render content
  const renderContent = () => {
    const table = streamQuery.getTable()

    if (streamQuery.isStreaming && !table) {
      return <LoadingState message="Loading data..." />
    }

    if (!table || table.numRows === 0) {
      return <EmptyState message="No processes available." />
    }

    const columns = table.schema.fields

    // Check if we have process_id for linking (needed for exe links)
    const hasProcessId = columns.some((f: Field) => f.name === 'process_id')
    const hasStartTime = columns.some((f: Field) => f.name === 'start_time')
    const hasLastUpdateTime = columns.some((f: Field) => f.name === 'last_update_time')

    return (
      <div className="flex-1 overflow-auto bg-app-panel border border-theme-border rounded-lg">
        <table className="w-full">
          <thead className="sticky top-0">
            <tr className="bg-app-card border-b border-theme-border">
              {columns.map((field: Field) => {
                const meta = getColumnMeta(field.name)
                return (
                  <SortHeader key={field.name} field={field.name} sortable={meta.sortable}>
                    {meta.label}
                  </SortHeader>
                )
              })}
            </tr>
          </thead>
          <tbody>
            {Array.from({ length: table.numRows }, (_, i) => {
              const row = table.get(i) as Record<string, unknown> | null
              if (!row) return null

              // Get process context for linking
              const processId = hasProcessId ? String(row.process_id ?? '') : ''
              const startDate = hasStartTime ? timestampToDate(row.start_time) : null
              const endDate = hasLastUpdateTime ? timestampToDate(row.last_update_time) : null
              const fromParam = startDate?.toISOString() ?? ''
              const toParam = endDate?.toISOString() ?? ''

              return (
                <tr
                  key={processId || i}
                  className="border-b border-theme-border hover:bg-app-card transition-colors"
                >
                  {columns.map((field: Field) => (
                    <td key={field.name} className="px-4 py-3">
                      {renderCell(field.name, field.type, row, processId, fromParam, toParam)}
                    </td>
                  ))}
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
