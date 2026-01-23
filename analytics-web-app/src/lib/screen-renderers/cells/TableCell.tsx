import { DataType } from 'apache-arrow'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { QueryCellConfig, CellConfig, CellState } from '../notebook-types'
import { formatTimestamp } from '@/lib/time-range'
import { timestampToDate, isTimeType, isNumericType, isBinaryType } from '@/lib/arrow-utils'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { substituteMacros, DEFAULT_SQL } from '../notebook-utils'

// =============================================================================
// Renderer Component
// =============================================================================

function formatCell(value: unknown, dataType: DataType): string {
  if (value === null || value === undefined) return '-'

  if (isTimeType(dataType)) {
    const date = timestampToDate(value, dataType)
    return date ? formatTimestamp(date) : '-'
  }

  if (isNumericType(dataType)) {
    if (typeof value === 'number') {
      return value.toLocaleString()
    }
    if (typeof value === 'bigint') {
      return value.toLocaleString()
    }
    return String(value)
  }

  if (DataType.isBool(dataType)) {
    return value ? 'true' : 'false'
  }

  if (isBinaryType(dataType)) {
    const bytes = value instanceof Uint8Array ? value : Array.isArray(value) ? value : null
    if (bytes) {
      const previewLen = Math.min(bytes.length, 32)
      let preview = ''
      for (let i = 0; i < previewLen; i++) {
        const b = bytes[i]
        preview += b >= 32 && b <= 126 ? String.fromCharCode(b) : '.'
      }
      const suffix = bytes.length > previewLen ? '...' : ''
      return `${preview}${suffix} (${bytes.length})`
    }
  }

  return String(value)
}

export function TableCell({ data, status }: CellRendererProps) {
  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center py-8">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
      </div>
    )
  }

  if (!data || data.numRows === 0) {
    return (
      <div className="text-center py-8 text-theme-text-muted text-sm">No data available</div>
    )
  }

  const columns = data.schema.fields.map((field) => ({
    name: field.name,
    type: field.type,
  }))

  return (
    <div className="overflow-auto h-full bg-app-bg border border-theme-border rounded-md">
      <table className="w-full text-sm">
        <thead className="sticky top-0">
          <tr className="bg-app-card border-b border-theme-border">
            {columns.map((col) => (
              <th
                key={col.name}
                className="px-3 py-2 text-left text-xs font-semibold uppercase tracking-wider text-theme-text-muted"
              >
                {col.name}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {Array.from({ length: Math.min(data.numRows, 100) }, (_, rowIdx) => {
            const row = data.get(rowIdx)
            if (!row) return null
            return (
              <tr
                key={rowIdx}
                className="border-b border-theme-border hover:bg-app-card/50 transition-colors"
              >
                {columns.map((col) => {
                  const value = row[col.name]
                  const formatted = formatCell(value, col.type)
                  return (
                    <td
                      key={col.name}
                      className="px-3 py-2 text-theme-text-primary font-mono truncate max-w-xs"
                      title={formatted}
                    >
                      {formatted}
                    </td>
                  )
                })}
              </tr>
            )
          })}
        </tbody>
      </table>
      {data.numRows > 100 && (
        <div className="px-3 py-2 text-xs text-theme-text-muted text-center bg-app-card border-t border-theme-border">
          Showing 100 of {data.numRows} rows
        </div>
      )}
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function TableCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const tableConfig = config as QueryCellConfig

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={tableConfig.sql}
          onChange={(sql) => onChange({ ...tableConfig, sql })}
          language="sql"
          placeholder="SELECT * FROM ..."
          minHeight="150px"
        />
      </div>
      <AvailableVariablesPanel variables={variables} timeRange={timeRange} />
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

// eslint-disable-next-line react-refresh/only-export-components
export const tableMetadata: CellTypeMetadata = {
  renderer: TableCell,
  EditorComponent: TableCellEditor,

  label: 'Table',
  icon: 'T',
  description: 'Generic SQL results as a table',
  showTypeBadge: true,
  defaultHeight: 300,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'table' as const,
    sql: DEFAULT_SQL.table,
  }),

  execute: async (config: CellConfig, { variables, timeRange, runQuery }: CellExecutionContext) => {
    const sql = substituteMacros((config as QueryCellConfig).sql, variables, timeRange)
    const data = await runQuery(sql)
    return { data }
  },

  getRendererProps: (_config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
  }),
}
