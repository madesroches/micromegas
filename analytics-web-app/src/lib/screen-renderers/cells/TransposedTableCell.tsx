import { useMemo } from 'react'
import { TableProperties } from 'lucide-react'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { QueryCellConfig, CellConfig, CellState } from '../notebook-types'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { DocumentationLink, QUERY_GUIDE_URL } from '@/components/DocumentationLink'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { substituteMacros, DEFAULT_SQL } from '../notebook-utils'
import { formatCell, HiddenColumnsBar, RowContextMenu, useRowManagement } from '../table-utils'

// =============================================================================
// Renderer Component
// =============================================================================

export function TransposedTableCell({ data, status, options, onOptionsChange }: CellRendererProps) {
  const table = data[0]

  const { hiddenRows, handleHideRow, handleRestoreRow, handleRestoreAll } = useRowManagement(
    options || {},
    onOptionsChange
  )

  const rows = useMemo(() => {
    if (!table || table.numRows === 0) return []
    return table.schema.fields.map((field) => ({
      name: field.name,
      type: field.type,
      values: Array.from({ length: table.numRows }, (_, i) => {
        const row = table.get(i)
        return row ? row[field.name] : null
      }),
    }))
  }, [table])

  const visibleRows = useMemo(() => {
    if (hiddenRows.length === 0) return rows
    const hiddenSet = new Set(hiddenRows)
    return rows.filter((row) => !hiddenSet.has(row.name))
  }, [rows, hiddenRows])

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center py-8">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
      </div>
    )
  }

  if (!table || table.numRows === 0) {
    return (
      <div className="text-center py-8 text-theme-text-muted text-sm">No data available</div>
    )
  }

  return (
    <div className="flex flex-col h-full">
      <HiddenColumnsBar
        hiddenColumns={hiddenRows}
        onRestore={handleRestoreRow}
        onRestoreAll={handleRestoreAll}
        compact
      />
      <div className="flex-1 overflow-auto min-h-0">
        <table className="w-full text-sm">
          <tbody>
            {visibleRows.map((row) => (
              <tr key={row.name} className="border-b border-theme-border">
                <RowContextMenu rowName={row.name} onHide={handleHideRow}>
                  <td className="px-3 py-1.5 text-theme-text-muted font-medium whitespace-nowrap align-top">
                    {row.name}
                  </td>
                </RowContextMenu>
                {row.values.map((value, colIdx) => (
                  <td key={colIdx} className="px-3 py-1.5 text-theme-text-primary">
                    {formatCell(value, row.type)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function TransposedTableCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const transposedConfig = config as QueryCellConfig

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={transposedConfig.sql}
          onChange={(sql) => onChange({ ...transposedConfig, sql })}
          language="sql"
          placeholder="SELECT * FROM ..."
          minHeight="150px"
        />
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
export const transposedTableMetadata: CellTypeMetadata = {
  renderer: TransposedTableCell,
  EditorComponent: TransposedTableCellEditor,

  label: 'Transposed',
  icon: <TableProperties />,
  description: 'SQL results in transposed key-value layout',
  showTypeBadge: true,
  defaultHeight: 300,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'transposed' as const,
    sql: DEFAULT_SQL.transposed,
  }),

  execute: async (config: CellConfig, { variables, timeRange, runQuery }: CellExecutionContext) => {
    const sql = substituteMacros((config as QueryCellConfig).sql, variables, timeRange)
    const data = await runQuery(sql)
    return { data: [data] }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
    options: (config as QueryCellConfig).options,
  }),
}
