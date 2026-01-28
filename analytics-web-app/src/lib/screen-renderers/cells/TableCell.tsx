import { useCallback } from 'react'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { QueryCellConfig, CellConfig, CellState } from '../notebook-types'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { substituteMacros, DEFAULT_SQL } from '../notebook-utils'
import { SortHeader, formatCell, buildOrderByClause, getNextSortState } from '../table-utils'

// =============================================================================
// Renderer Component
// =============================================================================

export function TableCell({ data, status, options, onOptionsChange }: CellRendererProps) {
  // Extract sort state from options
  const sortColumn = options?.sortColumn as string | undefined
  const sortDirection = options?.sortDirection as 'asc' | 'desc' | undefined

  // Three-state sort cycling: none -> ASC -> DESC -> none
  // Only update options - execution is triggered by useEffect watching options changes
  const handleSort = useCallback(
    (columnName: string) => {
      const nextState = getNextSortState(columnName, sortColumn, sortDirection)
      onOptionsChange({ ...options, ...nextState })
    },
    [sortColumn, sortDirection, options, onOptionsChange]
  )

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
              <SortHeader
                key={col.name}
                columnName={col.name}
                sortColumn={sortColumn}
                sortDirection={sortDirection}
                onSort={handleSort}
                compact
              >
                {col.name}
              </SortHeader>
            ))}
          </tr>
        </thead>
        <tbody>
          {Array.from({ length: data.numRows }, (_, rowIdx) => {
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
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function TableCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const tableConfig = config as QueryCellConfig

  // Compute the current $order_by value from sort state
  const sortColumn = tableConfig.options?.sortColumn as string | undefined
  const sortDirection = tableConfig.options?.sortDirection as 'asc' | 'desc' | undefined
  const orderByValue = buildOrderByClause(sortColumn, sortDirection) || '(click column headers to sort)'

  const tableVariables = [{ name: 'order_by', description: orderByValue }]

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
      <AvailableVariablesPanel
        variables={variables}
        timeRange={timeRange}
        additionalVariables={tableVariables}
      />
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
    const tableConfig = config as QueryCellConfig
    let sql = substituteMacros(tableConfig.sql, variables, timeRange)

    // Handle $order_by substitution based on sort state in options
    const sortColumn = tableConfig.options?.sortColumn as string | undefined
    const sortDirection = tableConfig.options?.sortDirection as 'asc' | 'desc' | undefined
    const orderByValue = buildOrderByClause(sortColumn, sortDirection)
    sql = sql.replace(/\$order_by/g, orderByValue)

    const data = await runQuery(sql)
    return { data }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
    options: (config as QueryCellConfig).options,
  }),
}
