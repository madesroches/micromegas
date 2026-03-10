import { useState, useCallback, useMemo, useEffect, useRef } from 'react'
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
import { RowSelectionEditor } from '@/components/RowSelectionEditor'
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

export function TableCell({ data, status, options, onOptionsChange, variables, timeRange, selectionMode, onSelectionChange }: CellRendererProps) {
  const table = data[0]

  // Extract overrides from options
  const overrides = (options?.overrides as ColumnOverride[] | undefined) || []

  // Selection state (ephemeral, like currentPage)
  const [selectedRowIndex, setSelectedRowIndex] = useState<number | null>(null)

  // Stable ref for onSelectionChange to avoid infinite re-render loop:
  // the callback is an inline arrow in NotebookRenderer, so including it
  // in the useEffect deps would fire the effect on every render.
  const onSelectionChangeRef = useRef(onSelectionChange)
  onSelectionChangeRef.current = onSelectionChange

  // Clear selection when data changes (re-execution)
  useEffect(() => {
    setSelectedRowIndex(null)
    onSelectionChangeRef.current?.(null)
  }, [table])

  const handleRowSelect = useCallback(
    (rowIndex: number | null) => {
      setSelectedRowIndex(rowIndex)
      if (!table || !onSelectionChange) return
      if (rowIndex === null) {
        onSelectionChange(null)
      } else {
        const row = table.get(rowIndex)
        if (row) {
          // Convert Arrow StructRow to plain object
          const obj: Record<string, unknown> = {}
          for (const field of table.schema.fields) {
            obj[field.name] = row[field.name]
          }
          onSelectionChange(obj)
        }
      }
    },
    [table, onSelectionChange],
  )

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

  const handlePageRelativeRowSelect = useCallback(
    (pageIdx: number | null) => {
      if (pageIdx === null) {
        handleRowSelect(null)
      } else {
        handleRowSelect(pagination.startRow + pageIdx)
      }
    },
    [handleRowSelect, pagination.startRow],
  )

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

  // Slice data for current page. Adjust selectedRowIndex for the page offset.
  const slicedData = {
    numRows: pagination.endRow - pagination.startRow,
    get: (index: number) => table.get(pagination.startRow + index),
  }

  // Map the absolute selectedRowIndex to the page-relative index
  const pageRelativeSelectedIndex =
    selectedRowIndex !== null &&
    selectedRowIndex >= pagination.startRow &&
    selectedRowIndex < pagination.endRow
      ? selectedRowIndex - pagination.startRow
      : null

  return (
    <div className="flex flex-col h-full">
      <HiddenColumnsBar hiddenColumns={hiddenColumns} onRestore={handleRestoreColumn} onRestoreAll={handleRestoreAll} compact />
      <div className="flex-1 overflow-auto min-h-0">
        <table className="w-full text-sm">
          <thead className="sticky top-0">
            <tr className="bg-app-card border-b-2 border-theme-border">
              {selectionMode === 'single' && (
                <th className="px-1 py-1.5 w-8" />
              )}
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
          <TableBody
            data={slicedData}
            columns={visibleColumns}
            allColumns={allColumns}
            compact
            overrides={overrides}
            variables={variables}
            timeRange={timeRange}
            selectionMode={selectionMode}
            selectedRowIndex={pageRelativeSelectedIndex}
            onRowSelect={handlePageRelativeRowSelect}
          />
        </table>
      </div>
      <PaginationBar pagination={pagination} />
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function TableCellEditor({ config, onChange, variables, timeRange, availableColumns, onRun, cellResults, cellSelections }: CellEditorProps) {
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

  // Selection mode
  const selectionMode = (tableConfig.options?.selectionMode as 'none' | 'single' | undefined) || 'none'
  const handleSelectionModeChange = useCallback(
    (mode: 'none' | 'single') => {
      onChange({
        ...tableConfig,
        options: { ...tableConfig.options, selectionMode: mode },
      })
    },
    [tableConfig, onChange]
  )

  // Validate macro references in SQL
  const validationErrors = useMemo(() => {
    const result = validateMacros(tableConfig.sql, variables, cellResults ?? {}, cellSelections ?? {})
    return result.errors
  }, [tableConfig.sql, variables, cellResults, cellSelections])

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
          onRunShortcut={onRun}
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
        cellResults={cellResults}
        cellSelections={cellSelections}
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
      <div className="mt-0">
        <RowSelectionEditor
          selectionMode={selectionMode}
          cellName={tableConfig.name}
          onChange={handleSelectionModeChange}
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

  execute: async (config: CellConfig, { variables, cellResults, cellSelections, timeRange, runQuery }: CellExecutionContext) => {
    const tableConfig = config as QueryCellConfig
    let sql = substituteMacros(tableConfig.sql, variables, timeRange, cellResults, cellSelections ?? {})

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
