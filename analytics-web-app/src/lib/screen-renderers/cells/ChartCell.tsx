import { useMemo, useCallback } from 'react'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { QueryCellConfig, CellConfig, CellState, VariableValue } from '../notebook-types'
import { XYChart, ScaleMode, ChartType } from '@/components/XYChart'
import { extractChartData } from '@/lib/arrow-utils'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { DocumentationLink, QUERY_GUIDE_URL } from '@/components/DocumentationLink'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { substituteMacros, DEFAULT_SQL } from '../notebook-utils'

/**
 * Substitutes macros in string values within chart options.
 * This allows using $variable.column syntax in options like unit labels.
 */
function substituteOptionsWithMacros(
  options: Record<string, unknown> | undefined,
  variables: Record<string, VariableValue>,
  timeRange: { begin: string; end: string }
): Record<string, unknown> {
  if (!options) return {}

  const result: Record<string, unknown> = {}

  for (const [key, value] of Object.entries(options)) {
    if (typeof value === 'string') {
      // Apply macro substitution to string values
      result[key] = substituteMacros(value, variables, timeRange)
    } else {
      // Keep non-string values as-is
      result[key] = value
    }
  }

  return result
}

// =============================================================================
// Renderer Component
// =============================================================================

export function ChartCell({ data, status, options, onOptionsChange, variables, timeRange }: CellRendererProps) {
  const chartResult = useMemo(() => {
    if (!data || data.numRows === 0) return null
    return extractChartData(data)
  }, [data])

  // Substitute macros in options (e.g., $variable.unit in the unit field)
  const resolvedOptions = useMemo(
    () => substituteOptionsWithMacros(options, variables, timeRange),
    [options, variables, timeRange]
  )

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
        scaleMode={(resolvedOptions?.scale_mode as ScaleMode) ?? 'p99'}
        onScaleModeChange={handleScaleModeChange}
        chartType={(resolvedOptions?.chart_type as ChartType) ?? 'line'}
        onChartTypeChange={handleChartTypeChange}
        unit={(resolvedOptions?.unit as string) ?? undefined}
      />
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function ChartCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const chartConfig = config as QueryCellConfig

  const handleOptionChange = (key: string, value: string) => {
    const newOptions = { ...chartConfig.options, [key]: value }
    onChange({ ...chartConfig, options: newOptions })
  }

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={chartConfig.sql}
          onChange={(sql) => onChange({ ...chartConfig, sql })}
          language="sql"
          placeholder="SELECT time, value FROM ..."
          minHeight="150px"
        />
      </div>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Y-Axis Unit
        </label>
        <input
          type="text"
          value={(chartConfig.options?.unit as string) ?? ''}
          onChange={(e) => handleOptionChange('unit', e.target.value)}
          className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
          placeholder="e.g., percent, bytes, ms, or $variable.unit"
        />
        <p className="mt-1 text-xs text-theme-text-muted">
          Use $variable.column for dynamic values (e.g., $selected_metric.unit)
        </p>
      </div>
      <AvailableVariablesPanel variables={variables} timeRange={timeRange} />
      <DocumentationLink url={QUERY_GUIDE_URL} label="Query Guide" />
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

// eslint-disable-next-line react-refresh/only-export-components
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
