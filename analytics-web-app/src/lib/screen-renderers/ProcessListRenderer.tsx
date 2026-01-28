import { useCallback, useEffect, useRef } from 'react'
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
import { formatTimestamp, formatDurationMs } from '@/lib/time-range'
import { timestampToDate, isTimeType, isDurationType, durationToMs } from '@/lib/arrow-utils'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useDefaultSaveCleanup } from '@/lib/url-cleanup-utils'

// Variables available for process list queries
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
  {
    name: 'order_by',
    description: 'ORDER BY clause or empty (click headers to cycle: none -> ASC -> DESC -> none)',
  },
]


interface ProcessListConfig {
  sql: string
  timeRangeFrom?: string
  timeRangeTo?: string
  sortColumn?: string
  sortDirection?: 'asc' | 'desc'
  [key: string]: unknown
}

interface SortHeaderProps {
  field: string
  children: React.ReactNode
  sortColumn?: string
  sortDirection?: 'asc' | 'desc'
  onSort: (field: string) => void
}

function SortHeader({
  field,
  children,
  sortColumn,
  sortDirection,
  onSort,
}: SortHeaderProps) {
  const isActive = sortColumn === field
  const showAsc = isActive && sortDirection === 'asc'
  const showDesc = isActive && sortDirection === 'desc'

  return (
    <th
      onClick={() => onSort(field)}
      className={`px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider cursor-pointer select-none transition-colors ${
        isActive
          ? 'text-theme-text-primary bg-app-card'
          : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-app-card'
      }`}
    >
      <div className="flex items-center gap-1">
        {children}
        {isActive && (
          <span className="text-accent-link flex-shrink-0">
            {showAsc ? (
              <ChevronUp className="w-3 h-3" />
            ) : showDesc ? (
              <ChevronDown className="w-3 h-3" />
            ) : null}
          </span>
        )}
      </div>
    </th>
  )
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

  // Sort state from config (persisted)
  const sortColumn = processListConfig.sortColumn
  const sortDirection = processListConfig.sortDirection

  // Query execution
  const streamQuery = useStreamQuery()
  const queryError = streamQuery.error?.message ?? null

  // Track current SQL for re-execution
  const currentSqlRef = useRef<string>(processListConfig.sql)
  const executeRef = useRef(streamQuery.execute)
  executeRef.current = streamQuery.execute

  // Build ORDER BY value from sort state
  const orderByValue =
    sortColumn && sortDirection ? `ORDER BY ${sortColumn} ${sortDirection.toUpperCase()}` : ''

  // Execute query with $order_by substitution
  const executeQuery = useCallback(
    (sql: string) => {
      currentSqlRef.current = sql

      executeRef.current({
        sql,
        params: {
          begin: timeRange.begin,
          end: timeRange.end,
          order_by: orderByValue,
        },
        begin: timeRange.begin,
        end: timeRange.end,
      })
    },
    [timeRange, orderByValue]
  )

  // Initial query execution
  const hasExecutedRef = useRef(false)
  useEffect(() => {
    if (!hasExecutedRef.current) {
      hasExecutedRef.current = true
      executeQuery(processListConfig.sql)
    }
  }, [executeQuery, processListConfig.sql])

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

  // Re-execute on refresh trigger
  const prevRefreshTriggerRef = useRef(refreshTrigger)
  useEffect(() => {
    if (prevRefreshTriggerRef.current !== refreshTrigger) {
      prevRefreshTriggerRef.current = refreshTrigger
      executeQuery(currentSqlRef.current)
    }
  }, [refreshTrigger, executeQuery])

  // Re-execute when sort changes (orderByValue changes)
  const prevOrderByRef = useRef<string | null>(null)
  useEffect(() => {
    if (prevOrderByRef.current === null) {
      prevOrderByRef.current = orderByValue
      return
    }
    if (prevOrderByRef.current !== orderByValue) {
      prevOrderByRef.current = orderByValue
      executeQuery(currentSqlRef.current)
    }
  }, [orderByValue, executeQuery])

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
    execute: (sql: string) => executeQuery(sql),
  })

  // Three-state sort cycling: none -> ASC -> DESC -> none
  const handleSort = useCallback(
    (columnName: string) => {
      let newSortColumn: string | undefined
      let newSortDirection: 'asc' | 'desc' | undefined

      if (sortColumn !== columnName) {
        // New column: start with ASC
        newSortColumn = columnName
        newSortDirection = 'asc'
      } else if (sortDirection === 'asc') {
        // ASC -> DESC
        newSortColumn = columnName
        newSortDirection = 'desc'
      } else {
        // DESC -> no sort (clear)
        newSortColumn = undefined
        newSortDirection = undefined
      }

      onConfigChange({
        ...processListConfig,
        sortColumn: newSortColumn,
        sortDirection: newSortDirection,
      })

      if (savedProcessListConfig) {
        const savedCol = savedProcessListConfig.sortColumn
        const savedDir = savedProcessListConfig.sortDirection
        setHasUnsavedChanges(newSortColumn !== savedCol || newSortDirection !== savedDir)
      }
    },
    [
      sortColumn,
      sortDirection,
      processListConfig,
      savedProcessListConfig,
      onConfigChange,
      setHasUnsavedChanges,
    ]
  )

  // Build currentValues with order_by for QueryEditor display
  const queryEditorValues = {
    ...currentValues,
    order_by: orderByValue || '(none)',
  }

  // Query editor panel
  const sqlPanel = (
    <QueryEditor
      defaultSql={savedProcessListConfig ? savedProcessListConfig.sql : processListConfig.sql}
      variables={VARIABLES}
      currentValues={queryEditorValues}
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
    executeQuery(currentSqlRef.current)
  }, [executeQuery])

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
        // Only render as link if we have a process_id to link to
        if (!processId) {
          return <span className="text-theme-text-primary">{String(value ?? '')}</span>
        }
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
        const date = timestampToDate(value, dataType)
        return (
          <span className="font-mono text-sm text-theme-text-primary">
            {date ? formatTimestamp(date) : '-'}
          </span>
        )
      }

      // Check if it's a duration type
      if (isDurationType(dataType)) {
        const ms = durationToMs(value, dataType)
        return (
          <span className="font-mono text-sm text-theme-text-secondary">
            {formatDurationMs(ms)}
          </span>
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
              {columns.map((field: Field) => (
                <SortHeader
                  key={field.name}
                  field={field.name}
                  sortColumn={sortColumn}
                  sortDirection={sortDirection}
                  onSort={handleSort}
                >
                  {field.name}
                </SortHeader>
              ))}
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
