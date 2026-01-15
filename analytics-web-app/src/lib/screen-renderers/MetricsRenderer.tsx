import { useState, useCallback, useEffect, useMemo } from 'react'
import { registerRenderer, ScreenRendererProps } from './index'
import { useScreenQuery } from './useScreenQuery'
import { useTimeRangeSync } from './useTimeRangeSync'
import { useSqlHandlers } from './useSqlHandlers'
import { LoadingState, EmptyState, SaveFooter, RendererLayout } from './shared'
import { QueryEditor } from '@/components/QueryEditor'
import { TimeSeriesChart, type ScaleMode } from '@/components/TimeSeriesChart'
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
  timeRangeFrom?: string
  timeRangeTo?: string
}

export function MetricsRenderer({
  config,
  onConfigChange,
  savedConfig,
  onUnsavedChange,
  timeRange,
  rawTimeRange,
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
  const savedMetricsConfig = savedConfig as MetricsConfig | null

  // Scale mode state - sync from config on load
  const [scaleMode, setScaleMode] = useState<ScaleMode>(
    metricsConfig.metrics_options?.scale_mode ?? 'p99'
  )

  // Query execution
  const query = useScreenQuery({
    initialSql: metricsConfig.sql,
    timeRange,
    refreshTrigger,
  })

  // Sync scale mode from config when loaded
  useEffect(() => {
    if (metricsConfig.metrics_options?.scale_mode) {
      setScaleMode(metricsConfig.metrics_options.scale_mode)
    }
  }, [metricsConfig.metrics_options?.scale_mode])

  // Sync time range changes to config
  useTimeRangeSync({
    rawTimeRange,
    savedConfig: savedMetricsConfig,
    config: metricsConfig,
    onUnsavedChange,
    onConfigChange,
  })

  // SQL editor handlers
  const { handleRunQuery, handleResetQuery, handleSqlChange } = useSqlHandlers({
    config: metricsConfig,
    savedConfig: savedMetricsConfig,
    onConfigChange,
    onUnsavedChange,
    execute: query.execute,
  })

  // Handle scale mode change - persists to config
  const handleScaleModeChange = useCallback(
    (mode: ScaleMode) => {
      setScaleMode(mode)
      const newConfig = {
        ...metricsConfig,
        metrics_options: { ...metricsConfig.metrics_options, scale_mode: mode },
      }
      onConfigChange(newConfig)

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
    const table = query.table
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
  }, [query.table])

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
      isLoading={query.isLoading}
      error={query.error}
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

  // Render content
  const renderContent = () => {
    if (query.isLoading && !query.table) {
      return <LoadingState message="Loading data..." />
    }

    if (!query.table || query.table.numRows === 0) {
      return <EmptyState message="No data available." />
    }

    if (chartData.length === 0) {
      return (
        <EmptyState message="No valid time/value data found. Query must return 'time' and 'value' columns." />
      )
    }

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

  return (
    <RendererLayout
      error={query.error}
      isRetryable={query.isRetryable}
      onRetry={query.retry}
      sqlPanel={sqlPanel}
    >
      {renderContent()}
    </RendererLayout>
  )
}

// Register this renderer
registerRenderer('metrics', MetricsRenderer)
