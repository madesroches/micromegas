import { useCallback, useMemo } from 'react'
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
import { substituteMacros, validateMacros, DEFAULT_SQL } from '../notebook-utils'
import {
  SortHeader,
  TableBody,
  HiddenColumnsBar,
  buildOrderByClause,
  getNextSortState,
  ColumnOverride,
} from '../table-utils'

// =============================================================================
// Renderer Component
// =============================================================================

export function TableCell({ data, status, options, onOptionsChange, variables }: CellRendererProps) {
  // Extract sort state, overrides, and hidden columns from options
  const sortColumn = options?.sortColumn as string | undefined
  const sortDirection = options?.sortDirection as 'asc' | 'desc' | undefined
  const overrides = (options?.overrides as ColumnOverride[] | undefined) || []
  const hiddenColumns = (options?.hiddenColumns as string[] | undefined) || []

  // Three-state sort cycling: none -> ASC -> DESC -> none
  // Only update options - execution is triggered by useEffect watching options changes
  const handleSort = useCallback(
    (columnName: string) => {
      const nextState = getNextSortState(columnName, sortColumn, sortDirection)
      onOptionsChange({ ...options, ...nextState })
    },
    [sortColumn, sortDirection, options, onOptionsChange]
  )

  const handleSortAsc = useCallback(
    (columnName: string) => {
      onOptionsChange({ ...options, sortColumn: columnName, sortDirection: 'asc' as const })
    },
    [options, onOptionsChange]
  )

  const handleSortDesc = useCallback(
    (columnName: string) => {
      onOptionsChange({ ...options, sortColumn: columnName, sortDirection: 'desc' as const })
    },
    [options, onOptionsChange]
  )

  const handleHideColumn = useCallback(
    (columnName: string) => {
      const hidden = (options?.hiddenColumns as string[] | undefined) || []
      if (hidden.includes(columnName)) return
      const updated: Record<string, unknown> = { ...options, hiddenColumns: [...hidden, columnName] }
      // Clear sort if the sorted column is being hidden
      if (sortColumn === columnName) {
        updated.sortColumn = undefined
        updated.sortDirection = undefined
      }
      onOptionsChange(updated)
    },
    [options, sortColumn, onOptionsChange]
  )

  const handleRestoreColumn = useCallback(
    (columnName: string) => {
      const hidden = (options?.hiddenColumns as string[] | undefined) || []
      onOptionsChange({ ...options, hiddenColumns: hidden.filter((c) => c !== columnName) })
    },
    [options, onOptionsChange]
  )

  const handleRestoreAll = useCallback(() => {
    onOptionsChange({ ...options, hiddenColumns: [] })
  }, [options, onOptionsChange])

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

  const allColumns = data.schema.fields.map((field) => ({
    name: field.name,
    type: field.type,
  }))
  const hiddenSet = new Set(hiddenColumns)
  const visibleColumns = allColumns.filter((c) => !hiddenSet.has(c.name))

  return (
    <div className="overflow-auto h-full bg-app-bg border border-theme-border rounded-md">
      <HiddenColumnsBar hiddenColumns={hiddenColumns} onRestore={handleRestoreColumn} onRestoreAll={handleRestoreAll} compact />
      <table className="w-full text-sm">
        <thead className="sticky top-0">
          <tr className="bg-app-card border-b border-theme-border">
            {visibleColumns.map((col) => (
              <SortHeader
                key={col.name}
                columnName={col.name}
                sortColumn={sortColumn}
                sortDirection={sortDirection}
                onSort={handleSort}
                onSortAsc={handleSortAsc}
                onSortDesc={handleSortDesc}
                onHide={handleHideColumn}
                compact
              >
                {col.name}
              </SortHeader>
            ))}
          </tr>
        </thead>
        <TableBody data={data} columns={visibleColumns} compact overrides={overrides} variables={variables} />
      </table>
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function TableCellEditor({ config, onChange, variables, timeRange, availableColumns }: CellEditorProps) {
  const tableConfig = config as QueryCellConfig

  // Compute the current $order_by value from sort state
  const sortColumn = tableConfig.options?.sortColumn as string | undefined
  const sortDirection = tableConfig.options?.sortDirection as 'asc' | 'desc' | undefined
  const orderByValue = buildOrderByClause(sortColumn, sortDirection) || '(click column headers to sort)'

  const tableVariables = [{ name: 'order_by', description: orderByValue }]

  // Get overrides from options
  const overrides = useMemo(
    () => (tableConfig.options?.overrides as ColumnOverride[] | undefined) || [],
    [tableConfig.options?.overrides]
  )

  // Handle overrides change
  const handleOverridesChange = useCallback(
    (newOverrides: ColumnOverride[]) => {
      onChange({
        ...tableConfig,
        options: { ...tableConfig.options, overrides: newOverrides },
      })
    },
    [tableConfig, onChange]
  )

  // Validate macro references in SQL
  const validationErrors = useMemo(() => {
    const result = validateMacros(tableConfig.sql, variables)
    return result.errors
  }, [tableConfig.sql, variables])

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
      {validationErrors.length > 0 && (
        <div className="text-red-400 text-sm space-y-1">
          {validationErrors.map((err, i) => (
            <div key={i}>⚠ {err}</div>
          ))}
        </div>
      )}
      <AvailableVariablesPanel
        variables={variables}
        timeRange={timeRange}
        additionalVariables={tableVariables}
      />
      <DocumentationLink url={QUERY_GUIDE_URL} label="Query Guide" />
      <div className="mt-4">
        <OverrideEditor
          overrides={overrides}
          availableColumns={availableColumns || []}
          availableVariables={Object.keys(variables)}
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
