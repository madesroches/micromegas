import { useCallback, useEffect, useRef } from 'react'
import { ChevronUp, ChevronDown } from 'lucide-react'
import { DataType } from 'apache-arrow'
import { registerRenderer, ScreenRendererProps } from './index'
import { useTimeRangeSync } from './useTimeRangeSync'
import { useSqlHandlers } from './useSqlHandlers'
import { LoadingState, EmptyState, SaveFooter, RendererLayout } from './shared'
import { QueryEditor } from '@/components/QueryEditor'
import { formatTimestamp } from '@/lib/time-range'
import { timestampToDate, isTimeType, isNumericType } from '@/lib/arrow-utils'
import { useStreamQuery } from '@/hooks/useStreamQuery'

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
}

/**
 * Check if an Arrow DataType is a binary type.
 */
function isBinaryType(dataType: DataType): boolean {
  return (
    DataType.isBinary(dataType) ||
    DataType.isLargeBinary(dataType) ||
    DataType.isFixedSizeBinary(dataType)
  )
}

interface SortHeaderProps {
  columnName: string
  children: React.ReactNode
  sortColumn?: string
  sortDirection?: 'asc' | 'desc'
  onSort: (columnName: string) => void
}

function SortHeader({
  columnName,
  children,
  sortColumn,
  sortDirection,
  onSort,
}: SortHeaderProps) {
  const isActive = sortColumn === columnName
  const showAsc = isActive && sortDirection === 'asc'
  const showDesc = isActive && sortDirection === 'desc'

  return (
    <th
      onClick={() => onSort(columnName)}
      className={`px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider cursor-pointer select-none transition-colors ${
        isActive
          ? 'text-theme-text-primary bg-app-card'
          : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-app-card'
      }`}
    >
      <div className="flex items-center gap-1">
        <span className="truncate">{children}</span>
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

/**
 * Format a cell value based on its Arrow DataType.
 */
function formatCell(value: unknown, dataType: DataType): string {
  if (value === null || value === undefined) return '-'

  if (isTimeType(dataType)) {
    const date = timestampToDate(value, dataType)
    return date ? formatTimestamp(date) : '-'
  }

  if (isNumericType(dataType)) {
    if (typeof value === 'number') {
      // Format with locale for readability
      return value.toLocaleString()
    }
    if (typeof value === 'bigint') {
      return value.toLocaleString()
    }
    return String(value)
  }

  if (DataType.isBool(dataType)) {
    return value ? 'true' : 'false'
  }

  // Binary data: display as ASCII preview with length
  if (isBinaryType(dataType)) {
    const bytes = value instanceof Uint8Array ? value : Array.isArray(value) ? value : null
    if (bytes) {
      const previewLen = Math.min(bytes.length, 32)
      let preview = ''
      for (let i = 0; i < previewLen; i++) {
        const b = bytes[i]
        // Printable ASCII range: 32-126
        preview += b >= 32 && b <= 126 ? String.fromCharCode(b) : '.'
      }
      const suffix = bytes.length > previewLen ? '...' : ''
      return `${preview}${suffix} (${bytes.length})`
    }
  }

  return String(value)
}

export function TableRenderer({
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
  const tableConfig = config as TableConfig
  const savedTableConfig = savedConfig as TableConfig | null

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
  const orderByValue =
    sortColumn && sortDirection
      ? `ORDER BY ${sortColumn} ${sortDirection.toUpperCase()}`
      : ''

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
    onUnsavedChange,
    onConfigChange,
  })

  // SQL editor handlers
  const { handleRunQuery, handleResetQuery, handleSqlChange } = useSqlHandlers({
    config: tableConfig,
    savedConfig: savedTableConfig,
    onConfigChange,
    onUnsavedChange,
    execute: (sql: string) => executeQuery(sql),
  })

  // Three-state sort cycling: none -> ASC -> DESC -> none
  const handleSort = useCallback(
    (columnName: string) => {
      if (sortColumn !== columnName) {
        // New column: start with ASC
        onConfigChange({ ...tableConfig, sortColumn: columnName, sortDirection: 'asc' })
      } else if (sortDirection === 'asc') {
        // ASC -> DESC
        onConfigChange({ ...tableConfig, sortDirection: 'desc' })
      } else {
        // DESC -> no sort (clear)
        onConfigChange({ ...tableConfig, sortColumn: undefined, sortDirection: undefined })
      }
      onUnsavedChange()
    },
    [sortColumn, sortDirection, tableConfig, onConfigChange, onUnsavedChange]
  )

  // Build currentValues with order_by for QueryEditor display
  const queryEditorValues = {
    ...currentValues,
    order_by: orderByValue || '(none)',
  }

  // Query editor panel
  const sqlPanel = (
    <QueryEditor
      defaultSql={savedConfig ? (savedConfig as TableConfig).sql : tableConfig.sql}
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
          <tbody>
            {Array.from({ length: table.numRows }, (_, rowIdx) => {
              const row = table.get(rowIdx)
              if (!row) return null
              return (
                <tr
                  key={rowIdx}
                  className="border-b border-theme-border hover:bg-app-card transition-colors"
                >
                  {columns.map((col) => {
                    const value = row[col.name]
                    // Use formatted value for tooltip on binary types
                    const formatted = formatCell(value, col.type)
                    const tooltip =
                      value != null
                        ? isBinaryType(col.type)
                          ? formatted
                          : String(value)
                        : undefined
                    return (
                      <td
                        key={col.name}
                        className="px-4 py-3 text-sm text-theme-text-primary font-mono truncate max-w-xs"
                        title={tooltip}
                      >
                        {formatted}
                      </td>
                    )
                  })}
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
registerRenderer('table', TableRenderer)
