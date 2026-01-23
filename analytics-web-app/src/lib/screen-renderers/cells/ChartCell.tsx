import { useMemo, useCallback } from 'react'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { QueryCellConfig, CellConfig, CellState } from '../notebook-types'
import { XYChart, ScaleMode, ChartType } from '@/components/XYChart'
import { extractChartData } from '@/lib/arrow-utils'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { substituteMacros, DEFAULT_SQL } from '../notebook-utils'

// =============================================================================
// Renderer Component
// =============================================================================

export function ChartCell({ data, status, options, onOptionsChange }: CellRendererProps) {
  const chartResult = useMemo(() => {
    if (!data || data.numRows === 0) return null
    return extractChartData(data)
  }, [data])

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
    <div className="h-full">
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

// =============================================================================
// Editor Component
// =============================================================================

function ChartCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const chartConfig = config as QueryCellConfig

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <textarea
          value={chartConfig.sql}
          onChange={(e) => onChange({ ...chartConfig, sql: e.target.value })}
          className="w-full min-h-[150px] px-3 py-2 bg-app-bg border border-theme-border rounded-md text-theme-text-primary text-sm font-mono focus:outline-none focus:border-accent-link resize-y"
          placeholder="SELECT time, value FROM ..."
        />
      </div>
      <AvailableVariablesPanel variables={variables} timeRange={timeRange} />
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

export const chartMetadata: CellTypeMetadata = {
  renderer: ChartCell,
  EditorComponent: ChartCellEditor,

  label: 'Chart',
  icon: 'C',
  description: 'X/Y chart (line, bar, etc.)',
  showTypeBadge: true,
  defaultHeight: 250,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'chart' as const,
    sql: DEFAULT_SQL.chart,
  }),

  execute: async (config: CellConfig, { variables, timeRange, runQuery }: CellExecutionContext) => {
    const sql = substituteMacros((config as QueryCellConfig).sql, variables, timeRange)
    const data = await runQuery(sql)
    return { data }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
    options: (config as QueryCellConfig).options,
  }),
}
