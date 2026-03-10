/* eslint-disable react-refresh/only-export-components */
import { useCallback, useMemo } from 'react'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { ReferenceTableCellConfig, CellConfig, CellState } from '../notebook-types'
import {
  SortHeader,
  TableBody,
  HiddenColumnsBar,
  useColumnManagement,
} from '../table-utils'
import { usePagination, PaginationBar, DEFAULT_PAGE_SIZE } from '../pagination'
import { csvToArrowIPC } from './csv-to-arrow'
import type { Table } from 'apache-arrow'
import { BookOpen } from 'lucide-react'

// =============================================================================
// Sorting Utility
// =============================================================================

export function buildSortedIndices(
  data: Table,
  sortColumn: string | undefined,
  sortDirection: string | undefined,
): number[] {
  const indices = Array.from({ length: data.numRows }, (_, i) => i)
  if (!sortColumn) return indices
  const col = data.getChild(sortColumn)
  if (!col) return indices
  const dir = sortDirection === 'desc' ? -1 : 1
  indices.sort((a, b) => {
    const va = col.get(a)
    const vb = col.get(b)
    const aNull = va == null || (typeof va === 'number' && isNaN(va))
    const bNull = vb == null || (typeof vb === 'number' && isNaN(vb))
    if (aNull && bNull) return 0
    if (aNull) return 1
    if (bNull) return -1
    if (va < vb) return -dir
    if (va > vb) return dir
    return 0
  })
  return indices
}

// =============================================================================
// Renderer Component
// =============================================================================

function ReferenceTableCell({ data, status, options, onOptionsChange, variables, timeRange }: CellRendererProps) {
  const table = data[0]

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

  const pageSize = (options?.pageSize as number | undefined) ?? DEFAULT_PAGE_SIZE
  const handlePageSizeChange = useCallback(
    (size: number) => onOptionsChange({ ...options, pageSize: size }),
    [options, onOptionsChange],
  )
  const pagination = usePagination(table?.numRows ?? 0, pageSize, handlePageSizeChange)

  const sortedIndices = useMemo(() => {
    if (!table || table.numRows === 0) return []
    return buildSortedIndices(table, sortColumn, sortDirection)
  }, [table, sortColumn, sortDirection])

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center py-8">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Parsing CSV...</span>
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

  const slicedData = {
    numRows: pagination.endRow - pagination.startRow,
    get: (index: number) => table.get(sortedIndices[pagination.startRow + index]),
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
          <TableBody data={slicedData} columns={visibleColumns} compact variables={variables} timeRange={timeRange} />
        </table>
      </div>
      <PaginationBar pagination={pagination} />
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function ReferenceTableCellEditor({ config, onChange }: CellEditorProps) {
  const refConfig = config as ReferenceTableCellConfig

  return (
    <div>
      <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
        CSV Data
      </label>
      <textarea
        value={refConfig.csv}
        onChange={(e) => onChange({ ...refConfig, csv: e.target.value })}
        className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm text-theme-text-primary font-mono resize-y min-h-[120px] focus:outline-none focus:border-accent-link"
        placeholder="column1,column2&#10;value1,value2"
        spellCheck={false}
      />
    </div>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

export const referenceTableMetadata: CellTypeMetadata = {
  renderer: ReferenceTableCell,
  EditorComponent: ReferenceTableCellEditor,

  label: 'Reference Table',
  icon: <BookOpen />,
  description: 'Inline CSV data as a queryable table',
  showTypeBadge: true,
  defaultHeight: 200,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'referencetable' as const,
    csv: 'column1,column2\nvalue1,value2',
  }),

  execute: async (config: CellConfig, context: CellExecutionContext) => {
    const refConfig = config as ReferenceTableCellConfig
    const { table, ipcBytes } = csvToArrowIPC(refConfig.csv)
    context.registerTable?.(ipcBytes)
    return { data: [table] }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
    options: (config as ReferenceTableCellConfig).options,
  }),
}
