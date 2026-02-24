import { useCallback, useMemo } from 'react'
import { Table2 } from 'lucide-react'
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
  useColumnManagement,
  ColumnOverride,
} from '../table-utils'
import { usePagination, PaginationBar, DEFAULT_PAGE_SIZE } from '../pagination'

// =============================================================================
// Renderer Component
// =============================================================================

export function TableCell({ data, status, options, onOptionsChange, variables }: CellRendererProps) {
  const table = data[0]

  // Extract overrides from options
  const overrides = (options?.overrides as ColumnOverride[] | undefined) || []

  // Column management (sort, hide/restore)
  const {
    sortColumn,
    sortDirection,
    hiddenColumns,
    handleSort,
    handleSortAsc,
    handleSortDesc,
    handleHideColumn,
    handleRestoreColumn,
    handleRestoreAll,
  } = useColumnManagement(options || {}, onOptionsChange)

  // Pagination
  const pageSize = (options?.pageSize as number | undefined) ?? DEFAULT_PAGE_SIZE
  const handlePageSizeChange = useCallback(
    (size: number) => onOptionsChange({ ...options, pageSize: size }),
    [options, onOptionsChange],
  )
  const pagination = usePagination(table?.numRows ?? 0, pageSize, handlePageSizeChange)

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

  const allColumns = table.schema.fields.map((field) => ({
    name: field.name,
    type: field.type,
  }))
  const hiddenSet = new Set(hiddenColumns)
  const visibleColumns = allColumns.filter((c) => !hiddenSet.has(c.name))

  // Slice data for current page
  const slicedData = {
    numRows: pagination.endRow - pagination.startRow,
    get: (index: number) => table.get(pagination.startRow + index),
  }

  return (
    <div className="flex flex-col h-full">
      <HiddenColumnsBar hiddenColumns={hiddenColumns} onRestore={handleRestoreColumn} onRestoreAll={handleRestoreAll} compact />
      <div className="flex-1 overflow-auto min-h-0">
        <table className="w-full text-sm">
          <thead className="sticky top-0">
            <tr className="bg-app-card border-b-2 border-theme-border">
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
          <TableBody data={slicedData} columns={visibleColumns} compact overrides={overrides} variables={variables} />
        </table>
      </div>
      <PaginationBar pagination={pagination} />
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
  icon: <Table2 />,
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
    return { data: [data] }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
    options: (config as QueryCellConfig).options,
  }),
}
