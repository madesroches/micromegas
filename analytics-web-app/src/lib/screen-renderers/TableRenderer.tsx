import { useCallback, useEffect, useRef, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { ChevronLeft, ChevronRight, ChevronDown, Play } from 'lucide-react'
import { registerRenderer, ScreenRendererProps } from './index'
import { useTimeRangeSync } from './useTimeRangeSync'
import { useSqlHandlers } from './useSqlHandlers'
import { LoadingState, EmptyState, RendererLayout } from './shared'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { OverrideEditor } from '@/components/OverrideEditor'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useDefaultSaveCleanup, useExposeSaveRef } from '@/lib/url-cleanup-utils'
import {
  SortHeader,
  TableBody,
  HiddenColumnsBar,
  buildOrderByClause,
  getNextSortState,
  ColumnOverride,
} from './table-utils'

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
  overrides?: ColumnOverride[]
  hiddenColumns?: string[]
  [key: string]: unknown
}

export function TableRenderer({
  config,
  onConfigChange,
  savedConfig,
  timeRange,
  rawTimeRange,
  timeRangeLabel,
  currentValues,
  onSave,
  refreshTrigger,
  onSaveRef,
}: ScreenRendererProps) {
  const tableConfig = config as unknown as TableConfig
  const savedTableConfig = savedConfig as unknown as TableConfig | null
  const [, setSearchParams] = useSearchParams()
  const handleSave = useDefaultSaveCleanup(onSave, setSearchParams)
  useExposeSaveRef(onSaveRef, handleSave)

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
    config: tableConfig,
    onConfigChange,
  })

  // SQL editor handlers
  const { handleRunQuery, handleResetQuery, handleSqlChange } = useSqlHandlers({
    config: tableConfig,
    savedConfig: savedTableConfig,
    onConfigChange,
    execute: (sql: string) => executeQuery(sql),
  })

  // Three-state sort cycling: none -> ASC -> DESC -> none
  const handleSort = useCallback(
    (columnName: string) => {
      const nextState = getNextSortState(columnName, sortColumn, sortDirection)
      onConfigChange({ ...tableConfig, ...nextState })
    },
    [sortColumn, sortDirection, tableConfig, onConfigChange]
  )

  const handleSortAsc = useCallback(
    (columnName: string) => {
      onConfigChange({ ...tableConfig, sortColumn: columnName, sortDirection: 'asc' as const })
    },
    [tableConfig, onConfigChange]
  )

  const handleSortDesc = useCallback(
    (columnName: string) => {
      onConfigChange({ ...tableConfig, sortColumn: columnName, sortDirection: 'desc' as const })
    },
    [tableConfig, onConfigChange]
  )

  // Handle overrides change
  const handleOverridesChange = useCallback(
    (newOverrides: ColumnOverride[]) => {
      onConfigChange({ ...tableConfig, overrides: newOverrides })
    },
    [tableConfig, onConfigChange]
  )

  // Hidden columns
  const hiddenColumns = tableConfig.hiddenColumns || []

  const handleHideColumn = useCallback(
    (columnName: string) => {
      const hidden = tableConfig.hiddenColumns || []
      if (hidden.includes(columnName)) return
      const updated = { ...tableConfig, hiddenColumns: [...hidden, columnName] }
      // Clear sort if the sorted column is being hidden
      if (tableConfig.sortColumn === columnName) {
        updated.sortColumn = undefined
        updated.sortDirection = undefined
      }
      onConfigChange(updated)
    },
    [tableConfig, onConfigChange]
  )

  const handleRestoreColumn = useCallback(
    (columnName: string) => {
      const hidden = tableConfig.hiddenColumns || []
      onConfigChange({ ...tableConfig, hiddenColumns: hidden.filter((c) => c !== columnName) })
    },
    [tableConfig, onConfigChange]
  )

  const handleRestoreAll = useCallback(() => {
    onConfigChange({ ...tableConfig, hiddenColumns: [] })
  }, [tableConfig, onConfigChange])

  // Get available columns from query result
  const table = streamQuery.getTable()
  const availableColumns = table ? table.schema.fields.map((f) => f.name) : []

  // Panel state
  const [isPanelCollapsed, setIsPanelCollapsed] = useState(true)
  const [isQueryExpanded, setIsQueryExpanded] = useState(true)
  const [sql, setSql] = useState(savedTableConfig ? savedTableConfig.sql : tableConfig.sql)

  const handleSqlRun = useCallback(() => {
    handleRunQuery(sql)
  }, [sql, handleRunQuery])

  const handleSqlReset = useCallback(() => {
    const defaultSql = savedTableConfig ? savedTableConfig.sql : tableConfig.sql
    setSql(defaultSql)
    handleResetQuery()
  }, [savedTableConfig, tableConfig.sql, handleResetQuery])

  // Query editor panel with overrides
  const sqlPanel = isPanelCollapsed ? (
    <div className="hidden md:flex w-12 bg-app-panel border-l border-theme-border flex-col">
      <div className="p-2">
        <button
          onClick={() => setIsPanelCollapsed(false)}
          className="w-8 h-8 flex items-center justify-center text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-border rounded transition-colors"
          title="Expand SQL Panel"
        >
          <ChevronLeft className="w-4 h-4" />
        </button>
      </div>
    </div>
  ) : (
    <div className="hidden md:flex w-80 lg:w-96 bg-app-panel border-l border-theme-border flex-col">
      {/* Panel Header */}
      <div className="flex items-center justify-between px-4 py-3 bg-app-card border-b border-theme-border">
        <div className="flex items-center gap-2">
          <button
            onClick={() => setIsPanelCollapsed(true)}
            className="w-6 h-6 flex items-center justify-center text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-border rounded transition-colors"
            title="Collapse panel"
          >
            <ChevronRight className="w-4 h-4" />
          </button>
          <span className="text-sm font-semibold text-theme-text-primary">Configuration</span>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={handleSqlReset}
            className="px-2.5 py-1 text-xs text-theme-text-secondary border border-theme-border rounded hover:bg-theme-border hover:text-theme-text-primary transition-colors"
          >
            Reset
          </button>
          <button
            onClick={handleSqlRun}
            disabled={streamQuery.isStreaming}
            className="flex items-center gap-1 px-2.5 py-1 text-xs bg-accent-success text-white rounded hover:opacity-90 disabled:bg-theme-border disabled:cursor-not-allowed transition-colors"
          >
            <Play className="w-3 h-3" />
            Run
          </button>
        </div>
      </div>

      {/* Scrollable Content */}
      <div className="flex-1 overflow-auto">
        {/* Query Section */}
        <div className="border-b border-theme-border">
          <button
            onClick={() => setIsQueryExpanded(!isQueryExpanded)}
            className="w-full flex items-center gap-2 px-4 py-2 bg-app-card/50 hover:bg-app-card transition-colors"
          >
            <ChevronDown
              className={`w-4 h-4 text-theme-text-muted transition-transform ${isQueryExpanded ? '' : '-rotate-90'}`}
            />
            <span className="text-sm font-semibold text-theme-text-primary">Query</span>
          </button>

          {isQueryExpanded && (
            <div className="p-4">
              <SyntaxEditor
                value={sql}
                onChange={(value) => {
                  setSql(value)
                  handleSqlChange(value)
                }}
                language="sql"
                minHeight="192px"
              />

              {/* Error */}
              {queryError && (
                <div className="mt-3 p-3 bg-accent-error/10 border border-accent-error/50 rounded-md">
                  <p className="text-xs text-accent-error">{queryError}</p>
                </div>
              )}

              {/* Variables */}
              <div className="mt-4">
                <h4 className="text-xs font-semibold uppercase tracking-wide text-theme-text-muted mb-2">Variables</h4>
                <div className="text-xs text-theme-text-muted space-y-1">
                  {VARIABLES.map((v) => (
                    <div key={v.name}>
                      <code className="px-1.5 py-0.5 bg-theme-border rounded text-accent-variable">${v.name}</code> -{' '}
                      {v.description}
                    </div>
                  ))}
                </div>
              </div>

              {/* Current Values */}
              <div className="mt-4">
                <h4 className="text-xs font-semibold uppercase tracking-wide text-theme-text-muted mb-2">
                  Current Values
                </h4>
                <div className="text-xs text-theme-text-muted space-y-1">
                  {Object.entries(currentValues).map(([key, value]) => (
                    <div key={key}>
                      <code className="px-1.5 py-0.5 bg-theme-border rounded text-accent-variable">${key}</code> ={' '}
                      <span className="text-theme-text-secondary">{value}</span>
                    </div>
                  ))}
                  <div>
                    <code className="px-1.5 py-0.5 bg-theme-border rounded text-accent-variable">$order_by</code> ={' '}
                    <span className="text-theme-text-secondary">{orderByValue || '(none)'}</span>
                  </div>
                </div>
              </div>

              {/* Time Range */}
              {timeRangeLabel && (
                <div className="mt-4">
                  <h4 className="text-xs font-semibold uppercase tracking-wide text-theme-text-muted mb-2">
                    Time Range
                  </h4>
                  <p className="text-xs text-theme-text-muted">
                    Applied implicitly via FlightSQL headers.
                    <br />
                    Current: <span className="text-theme-text-primary">{timeRangeLabel}</span>
                  </p>
                </div>
              )}
            </div>
          )}
        </div>

        {/* Overrides Section */}
        <OverrideEditor
          overrides={tableConfig.overrides || []}
          availableColumns={availableColumns}
          onChange={handleOverridesChange}
        />
      </div>

    </div>
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

    // Get columns from Arrow schema, filter out hidden
    const allColumns = table.schema.fields.map((field) => ({
      name: field.name,
      type: field.type,
    }))
    const hiddenSet = new Set(hiddenColumns)
    const visibleColumns = allColumns.filter((c) => !hiddenSet.has(c.name))

    return (
      <div className="flex-1 overflow-auto bg-app-panel border border-theme-border rounded-lg">
        <HiddenColumnsBar hiddenColumns={hiddenColumns} onRestore={handleRestoreColumn} onRestoreAll={handleRestoreAll} />
        <table className="w-full">
          <thead className="sticky top-0">
            <tr className="bg-app-card border-b border-theme-border">
              {visibleColumns.map((col) => (
                <SortHeader
                  key={col.name}
                  columnName={col.name}
                  sortColumn={sortColumn}
                  sortDirection={sortDirection}
                  onSort={handleSort}
                  onSortAsc={handleSortAsc}
                  onSortDesc={handleSortDesc}
                  onHide={handleHideColumn}
                >
                  {col.name}
                </SortHeader>
              ))}
            </tr>
          </thead>
          <TableBody data={table} columns={visibleColumns} overrides={tableConfig.overrides} />
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
