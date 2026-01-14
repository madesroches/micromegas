import { useCallback, useEffect, useRef } from 'react'
import { Save } from 'lucide-react'
import { registerRenderer, ScreenRendererProps } from './index'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { Button } from '@/components/ui/button'
import { useStreamQuery } from '@/hooks/useStreamQuery'

// Variables available for log queries
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
]

interface LogConfig {
  sql: string
}

export function LogRenderer({
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
  const logConfig = config as LogConfig

  // Query execution
  const streamQuery = useStreamQuery()
  const queryError = streamQuery.error?.message ?? null
  const table = streamQuery.getTable()

  // Refs for query execution
  const currentSqlRef = useRef<string>(logConfig.sql)
  const executeRef = useRef(streamQuery.execute)
  executeRef.current = streamQuery.execute

  // Execute query
  const loadData = useCallback(
    (sql: string) => {
      currentSqlRef.current = sql
      onConfigChange({ ...logConfig, sql })

      // Check if SQL changed from saved version
      if (savedConfig && sql !== (savedConfig as LogConfig).sql) {
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
    [logConfig, savedConfig, onConfigChange, onUnsavedChange, timeRange]
  )

  // Initial query execution
  const hasExecutedRef = useRef(false)
  useEffect(() => {
    if (!hasExecutedRef.current) {
      hasExecutedRef.current = true
      loadData(logConfig.sql)
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
      loadData((savedConfig as LogConfig).sql)
    } else {
      loadData(logConfig.sql)
    }
  }, [savedConfig, logConfig.sql, loadData])

  const handleSqlChange = useCallback(
    (sql: string) => {
      if (savedConfig && sql !== (savedConfig as LogConfig).sql) {
        onUnsavedChange()
      }
    },
    [savedConfig, onUnsavedChange]
  )

  // Query editor panel
  const sqlPanel = (
    <QueryEditor
      defaultSql={savedConfig ? (savedConfig as LogConfig).sql : logConfig.sql}
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
          <span className="text-theme-text-muted">No results</span>
        </div>
      )
    }

    // Generic data table
    return (
      <div className="flex-1 overflow-auto bg-app-panel border border-theme-border rounded-lg">
        <table className="w-full">
          <thead className="sticky top-0">
            <tr className="bg-app-card border-b border-theme-border">
              {table.schema.fields.map((field) => (
                <th
                  key={field.name}
                  className="px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider text-theme-text-muted"
                >
                  {field.name}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {Array.from({ length: table.numRows }, (_, i) => {
              const row = table.get(i)
              if (!row) return null
              return (
                <tr
                  key={i}
                  className="border-b border-theme-border hover:bg-app-card transition-colors"
                >
                  {table.schema.fields.map((field) => {
                    const value = row[field.name]
                    const displayValue =
                      value === null || value === undefined
                        ? ''
                        : typeof value === 'object'
                          ? JSON.stringify(value)
                          : String(value)
                    return (
                      <td
                        key={field.name}
                        className="px-4 py-3 text-sm text-theme-text-primary"
                      >
                        {displayValue}
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

  // Handle retry
  const handleRetry = useCallback(() => {
    loadData(currentSqlRef.current)
  }, [loadData])

  return (
    <div className="flex h-full">
      <div className="flex-1 flex flex-col p-6 overflow-hidden">
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
registerRenderer('log', LogRenderer)
