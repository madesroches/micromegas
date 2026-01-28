import { useCallback, useEffect, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { registerRenderer, ScreenRendererProps } from './index'
import { useTimeRangeSync } from './useTimeRangeSync'
import { useSqlHandlers } from './useSqlHandlers'
import { LoadingState, EmptyState, SaveFooter, RendererLayout } from './shared'
import { QueryEditor } from '@/components/QueryEditor'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useDefaultSaveCleanup } from '@/lib/url-cleanup-utils'
import { SortHeader, TableBody, buildOrderByClause, getNextSortState } from './table-utils'

// Variables available for table queries
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
  {
    name: 'order_by',
    description: 'ORDER BY clause or empty (click headers to cycle: none -> ASC -> DESC -> none)',
  },
]

interface TableConfig {
  sql: string
  timeRangeFrom?: string
  timeRangeTo?: string
  sortColumn?: string
  sortDirection?: 'asc' | 'desc'
  [key: string]: unknown
}

export function TableRenderer({
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
  const tableConfig = config as unknown as TableConfig
  const savedTableConfig = savedConfig as unknown as TableConfig | null
  const [, setSearchParams] = useSearchParams()
  const handleSave = useDefaultSaveCleanup(onSave, setSearchParams)

  // Sort state from config (persisted)
  const sortColumn = tableConfig.sortColumn
  const sortDirection = tableConfig.sortDirection

  // Query execution
  const streamQuery = useStreamQuery()
  const queryError = streamQuery.error?.message ?? null

  // Track current SQL for re-execution
  const currentSqlRef = useRef<string>(tableConfig.sql)
  const executeRef = useRef(streamQuery.execute)
  executeRef.current = streamQuery.execute

  // Build ORDER BY value from sort state
  const orderByValue = buildOrderByClause(sortColumn, sortDirection)

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
      executeQuery(tableConfig.sql)
    }
  }, [executeQuery, tableConfig.sql])

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
    savedConfig: savedTableConfig,
    config: tableConfig,
    setHasUnsavedChanges,
    onConfigChange,
  })

  // SQL editor handlers
  const { handleRunQuery, handleResetQuery, handleSqlChange } = useSqlHandlers({
    config: tableConfig,
    savedConfig: savedTableConfig,
    onConfigChange,
    setHasUnsavedChanges,
    execute: (sql: string) => executeQuery(sql),
  })

  // Three-state sort cycling: none -> ASC -> DESC -> none
  const handleSort = useCallback(
    (columnName: string) => {
      const nextState = getNextSortState(columnName, sortColumn, sortDirection)
      onConfigChange({ ...tableConfig, ...nextState })

      if (savedTableConfig) {
        const savedCol = savedTableConfig.sortColumn
        const savedDir = savedTableConfig.sortDirection
        setHasUnsavedChanges(nextState.sortColumn !== savedCol || nextState.sortDirection !== savedDir)
      }
    },
    [sortColumn, sortDirection, tableConfig, savedTableConfig, onConfigChange, setHasUnsavedChanges]
  )

  // Build currentValues with order_by for QueryEditor display
  const queryEditorValues = {
    ...currentValues,
    order_by: orderByValue || '(none)',
  }

  // Query editor panel
  const sqlPanel = (
    <QueryEditor
      defaultSql={savedTableConfig ? savedTableConfig.sql : tableConfig.sql}
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

  // Render content
  const renderContent = () => {
    const table = streamQuery.getTable()

    if (streamQuery.isStreaming && !table) {
      return <LoadingState message="Loading data..." />
    }

    if (!table || table.numRows === 0) {
      return <EmptyState message="No results for the current query." />
    }

    // Get columns from Arrow schema
    const columns = table.schema.fields.map((field) => ({
      name: field.name,
      type: field.type,
    }))

    return (
      <div className="flex-1 overflow-auto bg-app-panel border border-theme-border rounded-lg">
        <table className="w-full">
          <thead className="sticky top-0">
            <tr className="bg-app-card border-b border-theme-border">
              {columns.map((col) => (
                <SortHeader
                  key={col.name}
                  columnName={col.name}
                  sortColumn={sortColumn}
                  sortDirection={sortDirection}
                  onSort={handleSort}
                >
                  {col.name}
                </SortHeader>
              ))}
            </tr>
          </thead>
          <TableBody data={table} columns={columns} />
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
registerRenderer('table', TableRenderer)
