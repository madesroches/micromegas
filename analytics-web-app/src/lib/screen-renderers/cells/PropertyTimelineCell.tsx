import { useMemo, useCallback } from 'react'
import { Table } from 'apache-arrow'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { QueryCellConfig, CellConfig, CellState } from '../notebook-types'
import { PropertyTimeline } from '@/components/PropertyTimeline'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { DocumentationLink, QUERY_GUIDE_URL } from '@/components/DocumentationLink'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { substituteMacros, validateMacros, DEFAULT_SQL } from '../notebook-utils'
import { timestampToMs } from '@/lib/arrow-utils'
import { extractPropertiesFromRows, createPropertyTimelineGetter } from '@/lib/property-utils'
import { PropertyTimelineData } from '@/types'

// =============================================================================
// Data Transformation
// =============================================================================

/** Extract time and properties columns from Arrow table */
function extractRowsFromTable(table: Table): { time: number; properties: string | null }[] {
  const rows: { time: number; properties: string | null }[] = []
  const timeCol = table.getChild('time')
  const propsCol = table.getChild('properties')

  if (!timeCol) return rows

  const timeField = table.schema.fields.find(f => f.name === 'time')

  for (let i = 0; i < table.numRows; i++) {
    const time = timestampToMs(timeCol.get(i), timeField?.type)
    const properties = propsCol?.get(i) ?? null
    rows.push({ time, properties: properties != null ? String(properties) : null })
  }
  return rows
}

function transformToPropertyTimelines(
  table: Table,
  selectedKeys: string[],
  timeRange: { begin: number; end: number }
): { timelines: PropertyTimelineData[]; availableKeys: string[]; errors: string[] } {
  // 1. Extract rows as { time, properties } from Arrow table
  const rows = extractRowsFromTable(table)

  // 2. Parse JSON properties and collect available keys
  const { availableKeys, rawData, errors } = extractPropertiesFromRows(rows)

  // 3. Create getter and build timelines for selected keys
  const getTimeline = createPropertyTimelineGetter(rawData, timeRange)
  const timelines = selectedKeys.map(key => getTimeline(key))

  return { timelines, availableKeys, errors }
}

// =============================================================================
// Renderer Component
// =============================================================================

export function PropertyTimelineCell({
  data,
  status,
  options,
  onOptionsChange,
  timeRange,
}: CellRendererProps) {
  // Convert ISO time range to milliseconds
  const timeRangeMs = useMemo(() => ({
    begin: new Date(timeRange.begin).getTime(),
    end: new Date(timeRange.end).getTime(),
  }), [timeRange.begin, timeRange.end])

  // Extract selected keys from options
  const selectedKeys = useMemo(
    () => (options?.selectedKeys as string[]) ?? [],
    [options?.selectedKeys]
  )

  // Transform data to property timelines
  const { timelines, availableKeys, errors } = useMemo(() => {
    if (!data || data.numRows === 0) {
      return { timelines: [], availableKeys: [], errors: [] }
    }
    return transformToPropertyTimelines(data, selectedKeys, timeRangeMs)
  }, [data, selectedKeys, timeRangeMs])

  const handleAddProperty = useCallback(
    (key: string) => {
      onOptionsChange({ ...options, selectedKeys: [...selectedKeys, key] })
    },
    [options, selectedKeys, onOptionsChange]
  )

  const handleRemoveProperty = useCallback(
    (key: string) => {
      onOptionsChange({ ...options, selectedKeys: selectedKeys.filter(k => k !== key) })
    },
    [options, selectedKeys, onOptionsChange]
  )

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center h-[200px]">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
      </div>
    )
  }

  if (!data || data.numRows === 0) {
    return (
      <div className="flex items-center justify-center h-[200px] text-theme-text-muted text-sm">
        No data available
      </div>
    )
  }

  return (
    <div className="h-full flex flex-col">
      {/* Error warning banner */}
      {errors.length > 0 && (
        <div className="mb-2 px-3 py-2 bg-amber-500/10 border border-amber-500/30 rounded text-amber-400 text-xs">
          <span className="font-medium">Warning:</span> {errors.length} row(s) had invalid JSON properties and were skipped.
          <details className="mt-1">
            <summary className="cursor-pointer hover:text-amber-300">Show details</summary>
            <ul className="mt-1 ml-4 list-disc text-amber-400/80">
              {errors.slice(0, 5).map((err, i) => <li key={i}>{err}</li>)}
              {errors.length > 5 && <li>...and {errors.length - 5} more</li>}
            </ul>
          </details>
        </div>
      )}

      {/* Property timeline */}
      <PropertyTimeline
        properties={timelines}
        availableKeys={availableKeys}
        selectedKeys={selectedKeys}
        timeRange={{ from: timeRangeMs.begin, to: timeRangeMs.end }}
        onAddProperty={handleAddProperty}
        onRemoveProperty={handleRemoveProperty}
        showTimeAxis={true}
      />
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function PropertyTimelineCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const ptConfig = config as QueryCellConfig

  // Validate macro references in SQL
  const validationErrors = useMemo(() => {
    return validateMacros(ptConfig.sql, variables).errors
  }, [ptConfig.sql, variables])

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={ptConfig.sql}
          onChange={(sql) => onChange({ ...ptConfig, sql })}
          language="sql"
          placeholder="SELECT time, properties FROM ..."
          minHeight="150px"
        />
      </div>
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
export const propertyTimelineMetadata: CellTypeMetadata = {
  renderer: PropertyTimelineCell,
  EditorComponent: PropertyTimelineCellEditor,

  label: 'Property Timeline',
  icon: 'P',
  description: 'Display property values over time as horizontal segments',
  showTypeBadge: true,
  defaultHeight: 200,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'propertytimeline' as const,
    sql: DEFAULT_SQL.propertytimeline,
    options: {},
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
