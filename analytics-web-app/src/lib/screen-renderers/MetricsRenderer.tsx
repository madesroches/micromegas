import { useState, useCallback, useEffect, useMemo } from 'react'
import { useSearchParams } from 'react-router-dom'
import { registerRenderer, ScreenRendererProps } from './index'
import { useScreenQuery } from './useScreenQuery'
import { useTimeRangeSync } from './useTimeRangeSync'
import { useSqlHandlers } from './useSqlHandlers'
import { LoadingState, EmptyState, RendererLayout } from './shared'
import { QueryEditor } from '@/components/QueryEditor'
import { XYChart, type ScaleMode, type ChartType } from '@/components/XYChart'
import { extractChartData } from '@/lib/arrow-utils'
import { useDefaultSaveCleanup, useExposeSaveRef } from '@/lib/url-cleanup-utils'

// Variables available for metrics queries
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
]

interface MetricsOptions {
  scale_mode?: ScaleMode
  chart_type?: ChartType
}

interface MetricsConfig {
  sql: string
  metrics_options?: MetricsOptions
  timeRangeFrom?: string
  timeRangeTo?: string
  [key: string]: unknown
}

export function MetricsRenderer({
  config,
  onConfigChange,
  savedConfig,
  timeRange,
  rawTimeRange,
  onTimeRangeChange,
  timeRangeLabel,
  currentValues,
  onSave,
  refreshTrigger,
  onSaveRef,
  dataSource,
}: ScreenRendererProps) {
  const metricsConfig = config as unknown as MetricsConfig
  const savedMetricsConfig = savedConfig as unknown as MetricsConfig | null
  const [, setSearchParams] = useSearchParams()
  const handleSave = useDefaultSaveCleanup(onSave, setSearchParams)
  useExposeSaveRef(onSaveRef, handleSave)

  // Scale mode state - sync from config on load
  const [scaleMode, setScaleMode] = useState<ScaleMode>(
    metricsConfig.metrics_options?.scale_mode ?? 'p99'
  )

  // Chart type state - sync from config on load
  const [chartType, setChartType] = useState<ChartType>(
    metricsConfig.metrics_options?.chart_type ?? 'line'
  )

  // Query execution
  const query = useScreenQuery({
    initialSql: metricsConfig.sql,
    timeRange,
    refreshTrigger,
    dataSource,
  })

  // Sync scale mode from config when loaded
  useEffect(() => {
    if (metricsConfig.metrics_options?.scale_mode) {
      setScaleMode(metricsConfig.metrics_options.scale_mode)
    }
  }, [metricsConfig.metrics_options?.scale_mode])

  // Sync chart type from config when loaded
  useEffect(() => {
    if (metricsConfig.metrics_options?.chart_type) {
      setChartType(metricsConfig.metrics_options.chart_type)
    }
  }, [metricsConfig.metrics_options?.chart_type])

  // Sync time range changes to config
  useTimeRangeSync({
    rawTimeRange,
    config: metricsConfig,
    onConfigChange,
  })

  // SQL editor handlers
  const { handleRunQuery, handleResetQuery, handleSqlChange } = useSqlHandlers({
    config: metricsConfig,
    savedConfig: savedMetricsConfig,
    onConfigChange,
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
    },
    [metricsConfig, onConfigChange]
  )

  // Handle chart type change - persists to config
  const handleChartTypeChange = useCallback(
    (type: ChartType) => {
      setChartType(type)
      const newConfig = {
        ...metricsConfig,
        metrics_options: { ...metricsConfig.metrics_options, chart_type: type },
      }
      onConfigChange(newConfig)
    },
    [metricsConfig, onConfigChange]
  )

  // Handle time range selection from chart drag
  const handleTimeRangeSelect = useCallback(
    (from: Date, to: Date) => {
      onTimeRangeChange(from.toISOString(), to.toISOString())
    },
    [onTimeRangeChange]
  )

  // Extract chart data from Arrow table using the generic utility
  const chartResult = useMemo(() => {
    const table = query.table
    if (!table || table.numRows === 0) return null
    return extractChartData(table)
  }, [query.table])

  // Query editor panel
  const sqlPanel = (
    <QueryEditor
      defaultSql={savedMetricsConfig ? savedMetricsConfig.sql : metricsConfig.sql}
      variables={VARIABLES}
      currentValues={currentValues}
      timeRangeLabel={timeRangeLabel}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      onChange={handleSqlChange}
      isLoading={query.isLoading}
      error={query.error}
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

    if (!chartResult) {
      return <EmptyState message="No data available." />
    }

    if (!chartResult.ok) {
      return <EmptyState message={chartResult.error} />
    }

    const { data, xAxisMode, xLabels, xColumnName, yColumnName } = chartResult

    return (
      <div className="flex-1 min-h-[400px] h-full">
        <XYChart
          data={data}
          xAxisMode={xAxisMode}
          xLabels={xLabels}
          xColumnName={xColumnName}
          yColumnName={yColumnName}
          scaleMode={scaleMode}
          onScaleModeChange={handleScaleModeChange}
          chartType={chartType}
          onChartTypeChange={handleChartTypeChange}
          onTimeRangeSelect={xAxisMode === 'time' ? handleTimeRangeSelect : undefined}
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
