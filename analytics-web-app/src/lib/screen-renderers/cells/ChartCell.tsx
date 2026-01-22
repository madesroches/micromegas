import { useMemo, useCallback } from 'react'
import { CellRendererProps, registerCellRenderer } from '../cell-registry'
import { XYChart, ScaleMode, ChartType } from '@/components/XYChart'
import { extractChartData } from '@/lib/arrow-utils'

export function ChartCell({ data, status, options, onOptionsChange }: CellRendererProps) {
  // Extract chart data from Arrow table
  const chartResult = useMemo(() => {
    if (!data || data.numRows === 0) return null
    return extractChartData(data)
  }, [data])

  // Hooks must be called unconditionally (before any returns)
  const handleScaleModeChange = useCallback(
    (mode: ScaleMode) => {
      onOptionsChange({ ...options, scale_mode: mode })
    },
    [options, onOptionsChange]
  )

  const handleChartTypeChange = useCallback(
    (type: ChartType) => {
      onOptionsChange({ ...options, chart_type: type })
    },
    [options, onOptionsChange]
  )

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center h-[200px]">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
      </div>
    )
  }

  if (!data || data.numRows === 0 || !chartResult) {
    return (
      <div className="flex items-center justify-center h-[200px] text-theme-text-muted text-sm">
        No data available
      </div>
    )
  }

  if (!chartResult.ok) {
    return (
      <div className="flex items-center justify-center h-[200px] text-accent-error text-sm">
        {chartResult.error}
      </div>
    )
  }

  const { data: chartData, xAxisMode, xLabels, xColumnName, yColumnName } = chartResult

  return (
    <div className="h-[250px]">
      <XYChart
        data={chartData}
        xAxisMode={xAxisMode}
        xLabels={xLabels}
        xColumnName={xColumnName}
        yColumnName={yColumnName}
        scaleMode={(options?.scale_mode as ScaleMode) ?? 'p99'}
        onScaleModeChange={handleScaleModeChange}
        chartType={(options?.chart_type as ChartType) ?? 'line'}
        onChartTypeChange={handleChartTypeChange}
      />
    </div>
  )
}

// Register this cell renderer
registerCellRenderer('chart', ChartCell)
