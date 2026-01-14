import { useState, useCallback, useEffect, useRef } from 'react'
import { ChevronUp, ChevronDown, Save } from 'lucide-react'
import { registerRenderer, ScreenRendererProps } from './index'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { AppLink } from '@/components/AppLink'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { Button } from '@/components/ui/button'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { formatTimestamp, formatDuration } from '@/lib/time-range'
import { timestampToDate } from '@/lib/arrow-utils'

// Variables available for process list queries
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
]

// Sorting types
type ProcessSortField = 'exe' | 'start_time' | 'last_update_time' | 'runtime' | 'username' | 'computer'
type SortDirection = 'asc' | 'desc'

interface ProcessListConfig {
  sql: string
}

export function ProcessListRenderer({
  config,
  onConfigChange,
  savedConfig,
  onUnsavedChange,
  timeRange,
  timeRangeLabel,
  currentValues,
  onSave,
  isSaving,
  hasUnsavedChanges,
  onSaveAs,
  saveError,
  refreshTrigger,
}: ScreenRendererProps) {
  const processListConfig = config as ProcessListConfig

  // Query execution
  const streamQuery = useStreamQuery()
  const queryError = streamQuery.error?.message ?? null
  const table = streamQuery.getTable()

  // Sorting state (UI-only, not persisted)
  const [sortField, setSortField] = useState<ProcessSortField>('last_update_time')
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc')

  // Refs for query execution
  const currentSqlRef = useRef<string>(processListConfig.sql)
  const executeRef = useRef(streamQuery.execute)
  executeRef.current = streamQuery.execute

  // Execute query
  const loadData = useCallback(
    (sql: string) => {
      currentSqlRef.current = sql
      onConfigChange({ ...processListConfig, sql })

      // Check if SQL changed from saved version
      if (savedConfig && sql !== (savedConfig as ProcessListConfig).sql) {
        onUnsavedChange()
      }

      executeRef.current({
        sql,
        params: {
          begin: timeRange.begin,
          end: timeRange.end,
        },
        begin: timeRange.begin,
        end: timeRange.end,
      })
    },
    [processListConfig, savedConfig, onConfigChange, onUnsavedChange, timeRange]
  )

  // Initial query execution
  const hasExecutedRef = useRef(false)
  useEffect(() => {
    if (!hasExecutedRef.current) {
      hasExecutedRef.current = true
      loadData(processListConfig.sql)
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
      loadData(currentSqlRef.current)
    }
  }, [timeRange, loadData])

  // Re-execute on refresh trigger
  const prevRefreshTriggerRef = useRef(refreshTrigger)
  useEffect(() => {
    if (prevRefreshTriggerRef.current !== refreshTrigger) {
      prevRefreshTriggerRef.current = refreshTrigger
      loadData(currentSqlRef.current)
    }
  }, [refreshTrigger, loadData])

  const handleRunQuery = useCallback(
    (sql: string) => {
      loadData(sql)
    },
    [loadData]
  )

  const handleResetQuery = useCallback(() => {
    if (savedConfig) {
      loadData((savedConfig as ProcessListConfig).sql)
    } else {
      loadData(processListConfig.sql)
    }
  }, [savedConfig, processListConfig.sql, loadData])

  const handleSqlChange = useCallback(
    (sql: string) => {
      if (savedConfig && sql !== (savedConfig as ProcessListConfig).sql) {
        onUnsavedChange()
      }
    },
    [savedConfig, onUnsavedChange]
  )

  const handleSort = useCallback((field: ProcessSortField) => {
    if (sortField === field) {
      setSortDirection(sortDirection === 'asc' ? 'desc' : 'asc')
    } else {
      setSortField(field)
      setSortDirection('desc')
    }
  }, [sortField, sortDirection])

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
      defaultSql={savedConfig ? (savedConfig as ProcessListConfig).sql : processListConfig.sql}
      variables={VARIABLES}
      currentValues={currentValues}
      timeRangeLabel={timeRangeLabel}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      onChange={handleSqlChange}
      isLoading={streamQuery.isStreaming}
      error={queryError}
      footer={
        <>
          <div className="border-t border-theme-border p-3 flex gap-2">
            {onSave && (
              <Button
                variant="default"
                size="sm"
                onClick={onSave}
                disabled={isSaving || !hasUnsavedChanges}
                className="gap-1"
              >
                <Save className="w-4 h-4" />
                {isSaving ? 'Saving...' : 'Save'}
              </Button>
            )}
            <Button
              variant="outline"
              size="sm"
              onClick={onSaveAs}
              className="gap-1"
            >
              <Save className="w-4 h-4" />
              Save As
            </Button>
          </div>
          {saveError && (
            <div className="px-3 pb-3">
              <p className="text-xs text-accent-error">{saveError}</p>
            </div>
          )}
        </>
      }
    />
  )

  // Render content
  const renderContent = () => {
    // Loading state
    if (streamQuery.isStreaming && !table) {
      return (
        <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
          <div className="flex items-center gap-3">
            <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
            <span className="text-theme-text-secondary">Loading data...</span>
          </div>
        </div>
      )
    }

    // Empty state
    if (!table || table.numRows === 0) {
      return (
        <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
          <span className="text-theme-text-muted">No processes available.</span>
        </div>
      )
    }

    // Data table
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
                      href={`/process?id=${processId}&from=${encodeURIComponent(fromParam)}&to=${encodeURIComponent(toParam)}`}
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

  // Handle retry
  const handleRetry = useCallback(() => {
    loadData(currentSqlRef.current)
  }, [loadData])

  return (
    <div className="flex h-full">
      <div className="flex-1 flex flex-col p-6 min-w-0">
        {queryError && (
          <ErrorBanner
            title="Query execution failed"
            message={queryError}
            onRetry={streamQuery.error?.retryable ? handleRetry : undefined}
          />
        )}
        {renderContent()}
      </div>
      {sqlPanel}
    </div>
  )
}

// Register this renderer
registerRenderer('process_list', ProcessListRenderer)
