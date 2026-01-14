import { useState, useCallback, useEffect, useRef, useMemo } from 'react'
import { Save } from 'lucide-react'
import { registerRenderer, ScreenRendererProps } from './index'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { TimeSeriesChart, type ScaleMode } from '@/components/TimeSeriesChart'
import { Button } from '@/components/ui/button'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { timestampToMs } from '@/lib/arrow-utils'

// Variables available for metrics queries
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
]

interface MetricsOptions {
  scale_mode?: ScaleMode
}

interface MetricsConfig {
  sql: string
  metrics_options?: MetricsOptions
}

export function MetricsRenderer({
  config,
  onConfigChange,
  savedConfig,
  onUnsavedChange,
  timeRange,
  onTimeRangeChange,
  timeRangeLabel,
  currentValues,
  onSave,
  isSaving,
  hasUnsavedChanges,
  onSaveAs,
  saveError,
  refreshTrigger,
}: ScreenRendererProps) {
  const metricsConfig = config as MetricsConfig

  // Query execution
  const streamQuery = useStreamQuery()
  const queryError = streamQuery.error?.message ?? null
  const table = streamQuery.getTable()

  // Scale mode state - sync from config on load
  const [scaleMode, setScaleMode] = useState<ScaleMode>(
    metricsConfig.metrics_options?.scale_mode ?? 'p99'
  )

  // Refs for query execution
  const currentSqlRef = useRef<string>(metricsConfig.sql)
  const executeRef = useRef(streamQuery.execute)
  executeRef.current = streamQuery.execute

  // Sync scale mode from config when loaded
  useEffect(() => {
    if (metricsConfig.metrics_options?.scale_mode) {
      setScaleMode(metricsConfig.metrics_options.scale_mode)
    }
  }, [metricsConfig.metrics_options?.scale_mode])

  // Execute query
  const loadData = useCallback(
    (sql: string) => {
      currentSqlRef.current = sql
      onConfigChange({ ...metricsConfig, sql })

      // Check if SQL changed from saved version
      if (savedConfig && sql !== (savedConfig as MetricsConfig).sql) {
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
    [metricsConfig, savedConfig, onConfigChange, onUnsavedChange, timeRange]
  )

  // Initial query execution
  const hasExecutedRef = useRef(false)
  useEffect(() => {
    if (!hasExecutedRef.current) {
      hasExecutedRef.current = true
      loadData(metricsConfig.sql)
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
      loadData((savedConfig as MetricsConfig).sql)
    } else {
      loadData(metricsConfig.sql)
    }
  }, [savedConfig, metricsConfig.sql, loadData])

  const handleSqlChange = useCallback(
    (sql: string) => {
      if (savedConfig && sql !== (savedConfig as MetricsConfig).sql) {
        onUnsavedChange()
      }
    },
    [savedConfig, onUnsavedChange]
  )

  // Handle scale mode change - persists to config
  const handleScaleModeChange = useCallback(
    (mode: ScaleMode) => {
      setScaleMode(mode)
      const newConfig = {
        ...metricsConfig,
        metrics_options: { ...metricsConfig.metrics_options, scale_mode: mode },
      }
      onConfigChange(newConfig)

      // Check if scale mode changed from saved version
      if (savedConfig && (savedConfig as MetricsConfig).metrics_options?.scale_mode !== mode) {
        onUnsavedChange()
      }
    },
    [metricsConfig, savedConfig, onConfigChange, onUnsavedChange]
  )

  // Handle time range selection from chart drag
  const handleTimeRangeSelect = useCallback(
    (from: Date, to: Date) => {
      onTimeRangeChange(from.toISOString(), to.toISOString())
    },
    [onTimeRangeChange]
  )

  // Transform table data to chart format
  const chartData = useMemo(() => {
    if (!table || table.numRows === 0) return []
    const points: { time: number; value: number }[] = []

    for (let i = 0; i < table.numRows; i++) {
      const row = table.get(i)
      if (row) {
        const time = timestampToMs(row.time)
        const value = Number(row.value)
        if (!isNaN(time) && !isNaN(value)) {
          points.push({ time, value })
        }
      }
    }
    // Sort by time ascending - uPlot requires data in chronological order
    points.sort((a, b) => a.time - b.time)
    return points
  }, [table])

  // Query editor panel
  const sqlPanel = (
    <QueryEditor
      defaultSql={savedConfig ? (savedConfig as MetricsConfig).sql : metricsConfig.sql}
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
          <span className="text-theme-text-muted">No data available.</span>
        </div>
      )
    }

    // No valid chart data
    if (chartData.length === 0) {
      return (
        <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
          <span className="text-theme-text-muted">
            No valid time/value data found. Query must return &apos;time&apos; and &apos;value&apos; columns.
          </span>
        </div>
      )
    }

    // Chart
    return (
      <div className="flex-1 min-h-[400px] h-full">
        <TimeSeriesChart
          data={chartData}
          title=""
          unit=""
          scaleMode={scaleMode}
          onScaleModeChange={handleScaleModeChange}
          onTimeRangeSelect={handleTimeRangeSelect}
        />
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
registerRenderer('metrics', MetricsRenderer)
