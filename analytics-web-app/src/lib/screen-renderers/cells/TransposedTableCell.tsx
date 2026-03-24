import { useCallback, useMemo } from 'react'
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
import { OverrideEditor } from '@/components/OverrideEditor'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { substituteMacros, DEFAULT_SQL } from '../notebook-utils'
import {
  ColumnOverride,
  formatCell,
  HiddenColumnsBar,
  OverrideCell,
  RowContextMenu,
  TableColumn,
  useRowManagement,
} from '../table-utils'

// =============================================================================
// Renderer Component
// =============================================================================

const EMPTY_OPTIONS: Record<string, unknown> = {}

export function TransposedTableCell({ data, status, options, onOptionsChange, variables, timeRange, cellSelections, cellResults }: CellRendererProps) {
  const table = data[0]

  const { hiddenRows, handleHideRow, handleRestoreRow, handleRestoreAll } = useRowManagement(
    options || EMPTY_OPTIONS,
    onOptionsChange
  )

  const overrideMap = useMemo(() => {
    const overrides = (options?.overrides as ColumnOverride[] | undefined) || []
    const map = new Map<string, string>()
    for (const o of overrides) {
      map.set(o.column, o.format)
    }
    return map
  }, [options?.overrides])

  const columns: TableColumn[] = useMemo(() => {
    if (!table) return []
    return table.schema.fields.map((field) => ({ name: field.name, type: field.type }))
  }, [table])

  const originalRows = useMemo(() => {
    if (!table || table.numRows === 0) return []
    return Array.from({ length: table.numRows }, (_, i) => table.get(i) as Record<string, unknown>)
  }, [table])

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
                    {overrideMap.has(row.name) ? (
                      <OverrideCell
                        format={overrideMap.get(row.name)!}
                        row={originalRows[colIdx]}
                        columns={columns}
                        variables={variables}
                        timeRange={timeRange}
                        cellSelections={cellSelections}
                        cellResults={cellResults}
                      />
                    ) : (
                      formatCell(value, row.type)
                    )}
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

function TransposedTableCellEditor({ config, onChange, variables, timeRange, availableColumns, onRun, cellResults, cellSelections }: CellEditorProps) {
  const transposedConfig = config as QueryCellConfig

  const overrides = useMemo(
    () => (transposedConfig.options?.overrides as ColumnOverride[] | undefined) || [],
    [transposedConfig.options?.overrides]
  )

  const handleOverridesChange = useCallback(
    (newOverrides: ColumnOverride[]) => {
      onChange({
        ...transposedConfig,
        options: { ...transposedConfig.options, overrides: newOverrides },
      })
    },
    [transposedConfig, onChange]
  )

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
          onRunShortcut={onRun}
        />
      </div>
      <AvailableVariablesPanel variables={variables} timeRange={timeRange} cellResults={cellResults} cellSelections={cellSelections} />
      <DocumentationLink url={QUERY_GUIDE_URL} label="Query Guide" />
      <div className="mt-4">
        <OverrideEditor
          overrides={overrides}
          availableColumns={availableColumns || []}
          availableVariables={Object.keys(variables)}
          cellSelectionNames={Object.keys(cellSelections)}
          onChange={handleOverridesChange}
        />
      </div>
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

  execute: async (config: CellConfig, { variables, cellResults, cellSelections, timeRange, runQuery }: CellExecutionContext) => {
    const sql = substituteMacros((config as QueryCellConfig).sql, variables, timeRange, cellResults, cellSelections)
    const data = await runQuery(sql)
    return { data: [data] }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
    options: (config as QueryCellConfig).options,
  }),
}
