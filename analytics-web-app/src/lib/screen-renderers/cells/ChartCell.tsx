import { useMemo, useCallback } from 'react'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { CellConfigBase, CellConfig, CellState, VariableValue } from '../notebook-types'
import { SERIES_COLORS } from '@/components/chart-constants'
import { XYChart, ScaleMode, ChartType } from '@/components/XYChart'
import { extractChartData, extractMultiSeriesChartData } from '@/lib/arrow-utils'
import type { ChartSeriesData } from '@/lib/arrow-utils'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { DocumentationLink, QUERY_GUIDE_URL } from '@/components/DocumentationLink'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { DataSourceSelector } from '@/components/DataSourceSelector'
import { BarChart3 } from 'lucide-react'
import { substituteMacros, validateMacros, DEFAULT_SQL } from '../notebook-utils'

// =============================================================================
// Multi-Query Chart Types
// =============================================================================

export interface ChartQueryDef {
  name?: string
  sql: string
  unit?: string
  label?: string
  dataSource?: string
}

interface ChartCellConfigV1 extends CellConfigBase {
  type: 'chart'
  sql: string
  options?: {
    unit?: string
    scale_mode?: ScaleMode
    chart_type?: ChartType
    [key: string]: unknown
  }
  dataSource?: string
}

export interface ChartCellConfigV2 extends CellConfigBase {
  type: 'chart'
  version: 2
  queries: ChartQueryDef[]
  options?: {
    scale_mode?: ScaleMode
    chart_type?: ChartType
    [key: string]: unknown
  }
}

// eslint-disable-next-line react-refresh/only-export-components
export function migrateChartConfig(config: CellConfig): ChartCellConfigV2 {
  if ('version' in config && (config as { version: number }).version === 2) {
    return config as unknown as ChartCellConfigV2
  }
  const v1 = config as ChartCellConfigV1
  const { sql, dataSource, options: v1Options, ...rest } = v1
  return {
    ...rest,
    version: 2,
    queries: [{
      sql: sql ?? '',
      unit: v1Options?.unit as string | undefined,
      dataSource: dataSource,
    }],
    options: {
      scale_mode: v1Options?.scale_mode as ScaleMode | undefined,
      chart_type: v1Options?.chart_type as ChartType | undefined,
    },
  }
}

function queryTableName(cellName: string, queryName?: string): string {
  return queryName ? `${cellName}.${queryName}` : cellName
}

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

export function ChartCell({ data, status, options, onOptionsChange, variables, timeRange, onTimeRangeSelect }: CellRendererProps) {
  // Detect multi-series: more than one table in the data array
  const isMultiSeries = data.length > 1

  // Single-series result
  const chartResult = useMemo(() => {
    const table = data[0]
    if (isMultiSeries || !table || table.numRows === 0) return null
    return extractChartData(table)
  }, [data, isMultiSeries])

  // Multi-series result: build from data tables + options
  const multiResult = useMemo(() => {
    if (!isMultiSeries || data.length === 0) return null

    // Get query metadata from options (set by getRendererProps)
    const queryMeta = (options as Record<string, unknown>)?._queryMeta as
      { unit?: string; label?: string }[] | undefined

    const tableInputs = data.map((table, i) => ({
      table,
      unit: queryMeta?.[i]?.unit,
      label: queryMeta?.[i]?.label,
    }))

    return extractMultiSeriesChartData(tableInputs)
  }, [data, isMultiSeries, options])

  // Substitute macros in options
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

  // Multi-series path
  if (isMultiSeries) {
    if (!multiResult) {
      return (
        <div className="flex items-center justify-center h-[200px] text-theme-text-muted text-sm">
          No data available
        </div>
      )
    }
    if (!multiResult.ok) {
      return (
        <div className="flex items-center justify-center h-[200px] text-accent-error text-sm">
          {multiResult.error}
        </div>
      )
    }

    // Resolve macros in per-series units
    const resolvedSeries: ChartSeriesData[] = multiResult.series.map(s => ({
      ...s,
      unit: s.unit ? substituteMacros(s.unit, variables, timeRange) : '',
    }))

    return (
      <div className="h-full">
        <XYChart
          series={resolvedSeries}
          xAxisMode={multiResult.xAxisMode}
          xLabels={multiResult.xLabels}
          xColumnName={multiResult.xColumnName}
          scaleMode={(resolvedOptions?.scale_mode as ScaleMode) ?? 'p99'}
          onScaleModeChange={handleScaleModeChange}
          chartType={(resolvedOptions?.chart_type as ChartType) ?? 'line'}
          onChartTypeChange={handleChartTypeChange}
          onTimeRangeSelect={onTimeRangeSelect}
        />
      </div>
    )
  }

  // Single-series path
  if (!data[0] || data[0].numRows === 0 || !chartResult) {
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

  // Extract per-query unit and label for single-series charts
  const singleQueryMeta = (options as Record<string, unknown>)?._queryMeta as
    { unit?: string; label?: string }[] | undefined
  const chartUnit = singleQueryMeta?.[0]?.unit
    ? substituteMacros(singleQueryMeta[0].unit, variables, timeRange)
    : (resolvedOptions?.unit as string) ?? undefined
  const chartTitle = singleQueryMeta?.[0]?.label || undefined

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
        unit={chartUnit}
        title={chartTitle}
        onTimeRangeSelect={onTimeRangeSelect}
      />
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function ChartCellEditor({ config, onChange, variables, timeRange, datasourceVariables, defaultDataSource, onRun }: CellEditorProps) {
  // Always work with v2 format
  const v2 = useMemo(() => migrateChartConfig(config), [config])

  const updateConfig = useCallback((updates: Partial<ChartCellConfigV2>) => {
    onChange({ ...v2, ...updates } as unknown as CellConfig)
  }, [v2, onChange])

  const updateQuery = useCallback((index: number, updates: Partial<ChartQueryDef>) => {
    const newQueries = [...v2.queries]
    newQueries[index] = { ...newQueries[index], ...updates }
    updateConfig({ queries: newQueries })
  }, [v2.queries, updateConfig])

  const addQuery = useCallback(() => {
    const newQuery: ChartQueryDef = {
      sql: DEFAULT_SQL.chart,
      dataSource: v2.queries[0]?.dataSource,
    }
    updateConfig({ queries: [...v2.queries, newQuery] })
  }, [v2.queries, updateConfig])

  const removeQuery = useCallback((index: number) => {
    if (v2.queries.length <= 1) return
    const newQueries = v2.queries.filter((_, i) => i !== index)
    updateConfig({ queries: newQueries })
  }, [v2.queries, updateConfig])

  // Validate macro references across all queries
  const validationErrors = useMemo(() => {
    const errors: string[] = []
    for (let i = 0; i < v2.queries.length; i++) {
      const q = v2.queries[i]
      const sqlValidation = validateMacros(q.sql, variables)
      sqlValidation.errors.forEach(e => errors.push(`Query ${i + 1}: ${e}`))
      if (q.unit) {
        const unitValidation = validateMacros(q.unit, variables)
        unitValidation.errors.forEach(e => errors.push(`Query ${i + 1} unit: ${e}`))
      }
    }
    return errors
  }, [v2.queries, variables])

  return (
    <>
      {v2.queries.map((query, i) => (
        <div key={i} className="bg-app-card border border-theme-border rounded-lg overflow-hidden">
          {/* Query block header */}
          <div className="flex justify-between items-center px-3 py-2 bg-app-panel border-b border-theme-border">
            <div className="flex items-center gap-2 text-xs font-medium text-theme-text-secondary">
              <div
                className="w-2 h-2 rounded-full"
                style={{ background: SERIES_COLORS[i % SERIES_COLORS.length] }}
              />
              Query {i + 1}
            </div>
            {v2.queries.length > 1 && (
              <button
                onClick={() => removeQuery(i)}
                className="text-theme-text-muted hover:text-accent-error text-base px-1.5 rounded transition-colors"
                title="Remove query"
              >
                &times;
              </button>
            )}
          </div>

          {/* Query block body */}
          <div className="p-3 space-y-3">
            {/* Data source per query */}
            <div>
              <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
                Data Source
              </label>
              <DataSourceSelector
                value={query.dataSource || defaultDataSource || ''}
                onChange={(ds) => updateQuery(i, { dataSource: ds })}
                datasourceVariables={datasourceVariables}
                showNotebookOption={true}
              />
            </div>

            {/* SQL editor */}
            <div>
              <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
                SQL Query
              </label>
              <SyntaxEditor
                value={query.sql}
                onChange={(sql) => updateQuery(i, { sql })}
                language="sql"
                placeholder="SELECT time, value FROM ..."
                minHeight="80px"
                onRunShortcut={onRun}
              />
            </div>

            {/* Inline fields: Unit and Label */}
            <div className="flex gap-3">
              <div className="flex-1">
                <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
                  Unit
                </label>
                <input
                  type="text"
                  value={query.unit ?? ''}
                  onChange={(e) => updateQuery(i, { unit: e.target.value })}
                  className="w-full px-3 py-1.5 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-xs focus:outline-none focus:border-accent-link"
                  placeholder="e.g., percent, bytes, ms"
                />
              </div>
              <div className="flex-1">
                <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
                  Label
                </label>
                <input
                  type="text"
                  value={query.label ?? ''}
                  onChange={(e) => updateQuery(i, { label: e.target.value })}
                  className="w-full px-3 py-1.5 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-xs focus:outline-none focus:border-accent-link"
                  placeholder="defaults to column name"
                />
              </div>
            </div>
          </div>
        </div>
      ))}

      {/* Add query button */}
      <button
        onClick={addQuery}
        className="w-full py-2.5 border border-dashed border-theme-border rounded-lg text-theme-text-muted text-sm hover:border-accent-link hover:text-accent-link transition-colors"
      >
        + Add Query
      </button>

      {validationErrors.length > 0 && (
        <div className="text-red-400 text-sm space-y-1">
          {validationErrors.map((err, i) => (
            <div key={i}>⚠ {err}</div>
          ))}
        </div>
      )}
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
  icon: <BarChart3 />,
  description: 'X/Y chart (line, bar, etc.)',
  showTypeBadge: true,
  defaultHeight: 250,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'chart' as const,
    sql: DEFAULT_SQL.chart,
  }),

  execute: async (config: CellConfig, { variables, timeRange, runQuery, runQueryAs }: CellExecutionContext) => {
    const v2 = migrateChartConfig(config)

    if (v2.queries.length <= 1) {
      // Single query path — always use runQueryAs when available so per-query
      // dataSource is respected (v2 configs have no top-level dataSource)
      const sql = substituteMacros(v2.queries[0]?.sql ?? '', variables, timeRange)
      if (runQueryAs) {
        const tableName = queryTableName(config.name, v2.queries[0]?.name)
        const table = await runQueryAs(sql, tableName, v2.queries[0]?.dataSource)
        return { data: [table] }
      }
      const table = await runQuery(sql)
      return { data: [table] }
    }

    // Multi-query execution — return flat array of tables
    const tables: import('apache-arrow').Table[] = []
    for (const query of v2.queries) {
      const sql = substituteMacros(query.sql, variables, timeRange)
      if (runQueryAs) {
        const tableName = queryTableName(config.name, query.name)
        tables.push(await runQueryAs(sql, tableName, query.dataSource))
      } else {
        tables.push(await runQuery(sql))
      }
    }

    return { data: tables }
  },

  getRendererProps: (config: CellConfig, state: CellState) => {
    const v2 = migrateChartConfig(config)
    // Pass query metadata (units/labels) through options so the renderer can build series
    const queryMeta = v2.queries.map(q => ({ unit: q.unit, label: q.label }))
    return {
      data: state.data,
      status: state.status,
      options: { ...v2.options, _queryMeta: queryMeta },
    }
  },
}
