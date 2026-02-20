import React, { useMemo, useCallback } from 'react'
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
import { usePagination, PaginationBar, DEFAULT_PAGE_SIZE } from '../pagination'
import { classifyLogColumns, renderLogColumn } from '../log-utils'

// =============================================================================
// Renderer Component
// =============================================================================

export function LogCell({ data, status, options, onOptionsChange }: CellRendererProps) {
  const table = data[0]

  const columns = useMemo(() => {
    if (!table) return []
    return classifyLogColumns(table.schema.fields)
  }, [table])

  const numRows = table?.numRows ?? 0

  // Pagination
  const pageSize = (options?.pageSize as number | undefined) ?? DEFAULT_PAGE_SIZE
  const handlePageSizeChange = useCallback(
    (size: number) => onOptionsChange({ ...options, pageSize: size }),
    [options, onOptionsChange],
  )
  const pagination = usePagination(numRows, pageSize, handlePageSizeChange)

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center py-8">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
      </div>
    )
  }

  if (numRows === 0) {
    return (
      <div className="text-center py-8 text-theme-text-muted text-sm">No log entries found</div>
    )
  }

  return (
    <div className="flex flex-col h-full font-mono text-[12px]">
      <div className="flex-1 overflow-auto min-h-0">
        {Array.from({ length: pagination.endRow - pagination.startRow }, (_, i) => {
          const rowIdx = pagination.startRow + i
          const row = table!.get(rowIdx)
          if (!row) return null
          return (
            <div
              key={rowIdx}
              className={`flex px-2 py-0.5 hover:bg-app-card/50 transition-colors${i % 2 === 0 ? '' : ' bg-app-card/30'}`}
            >
              {columns.map((col) => (
                <React.Fragment key={col.name}>{renderLogColumn(col, row)}</React.Fragment>
              ))}
            </div>
          )
        })}
      </div>
      <PaginationBar pagination={pagination} />
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function LogCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const logConfig = config as QueryCellConfig

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={logConfig.sql}
          onChange={(sql) => onChange({ ...logConfig, sql })}
          language="sql"
          placeholder="SELECT time, level, target, msg FROM log_entries ..."
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
export const logMetadata: CellTypeMetadata = {
  renderer: LogCell,
  EditorComponent: LogCellEditor,

  label: 'Log',
  icon: 'L',
  description: 'Log entries viewer with levels',
  showTypeBadge: true,
  defaultHeight: 300,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'log' as const,
    sql: DEFAULT_SQL.log,
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
