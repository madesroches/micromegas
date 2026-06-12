import React, { useMemo, useCallback, useState, useRef, useEffect } from 'react'
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
import {
  classifyLogColumns,
  renderLogColumn,
  computeFlexWidths,
  LogDivider,
} from '../log-utils'
import { ScrollText } from 'lucide-react'

const MIN_COL_WIDTH_PX = 40
const MAX_COL_WIDTH_PX = 1200

interface DragState {
  col: string
  startX: number
  startWidth: number
}

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

  const autoWidths = useMemo(
    () => computeFlexWidths(table, columns, pagination.startRow, pagination.endRow),
    [table, columns, pagination.startRow, pagination.endRow],
  )

  // -------------------------------------------------------------------------
  // Pinned widths state
  // -------------------------------------------------------------------------

  const [livePinnedWidths, setLivePinnedWidths] = useState<Record<string, number>>(
    () => (options?.columnWidths as Record<string, number> | undefined) ?? {},
  )

  // keep a ref in sync so mouseup handler reads current value without stale closure
  const livePinnedWidthsRef = useRef<Record<string, number>>(livePinnedWidths)

  const setAndSyncWidths = useCallback(
    (updater: (prev: Record<string, number>) => Record<string, number>) => {
      setLivePinnedWidths((prev) => {
        const next = updater(prev)
        livePinnedWidthsRef.current = next
        return next
      })
    },
    [],
  )

  // keep options ref current so mouseup spread doesn't clobber concurrent option changes
  const optionsRef = useRef(options)
  useEffect(() => {
    optionsRef.current = options
  })

  // Sync from outside (notebook reload) — guard by JSON equality to avoid clobbering drags
  const pinnedWidthsFromOptions = (options?.columnWidths as Record<string, number> | undefined) ?? {}
  const serializedWidthsFromOptions = JSON.stringify(pinnedWidthsFromOptions)
  const lastSyncedRef = useRef<string>(serializedWidthsFromOptions)
  useEffect(() => {
    if (serializedWidthsFromOptions !== lastSyncedRef.current) {
      lastSyncedRef.current = serializedWidthsFromOptions
      setAndSyncWidths(() => JSON.parse(serializedWidthsFromOptions))
    }
  }, [serializedWidthsFromOptions, setAndSyncWidths])

  // -------------------------------------------------------------------------
  // Drag state
  // -------------------------------------------------------------------------

  const dragRef = useRef<DragState | null>(null)
  const dragListenersRef = useRef<{
    onMouseMove: ((e: MouseEvent) => void) | null
    onMouseUp: (() => void) | null
  }>({ onMouseMove: null, onMouseUp: null })

  // Remove document listeners if the component unmounts during an active drag.
  useEffect(() => {
    return () => {
      const { onMouseMove, onMouseUp } = dragListenersRef.current
      if (onMouseMove) document.removeEventListener('mousemove', onMouseMove)
      if (onMouseUp) document.removeEventListener('mouseup', onMouseUp)
    }
  }, [])

  const handleDividerMouseDown = useCallback(
    (col: string, e: React.MouseEvent) => {
      e.preventDefault()
      // Cancel any in-progress drag before starting a new one
      const prev = dragListenersRef.current
      if (prev.onMouseMove) document.removeEventListener('mousemove', prev.onMouseMove)
      if (prev.onMouseUp) document.removeEventListener('mouseup', prev.onMouseUp)
      const effectiveWidth =
        livePinnedWidthsRef.current[col] ?? autoWidths[col] ?? MIN_COL_WIDTH_PX
      dragRef.current = { col, startX: e.clientX, startWidth: effectiveWidth }

      const onMouseMove = (me: MouseEvent) => {
        if (!dragRef.current) return
        const delta = me.clientX - dragRef.current.startX
        const col = dragRef.current.col
        const newWidth = Math.min(
          Math.max(dragRef.current.startWidth + delta, MIN_COL_WIDTH_PX),
          MAX_COL_WIDTH_PX,
        )
        setAndSyncWidths((prev) => ({ ...prev, [col]: newWidth }))
      }

      const onMouseUp = () => {
        document.removeEventListener('mousemove', onMouseMove)
        document.removeEventListener('mouseup', onMouseUp)
        dragListenersRef.current = { onMouseMove: null, onMouseUp: null }
        if (!dragRef.current) return
        const currentOptions = optionsRef.current
        onOptionsChange({
          ...currentOptions,
          columnWidths: { ...livePinnedWidthsRef.current },
        })
        dragRef.current = null
      }

      dragListenersRef.current = { onMouseMove, onMouseUp }
      document.addEventListener('mousemove', onMouseMove)
      document.addEventListener('mouseup', onMouseUp)
    },
    [autoWidths, onOptionsChange, setAndSyncWidths],
  )

  // -------------------------------------------------------------------------
  // Hovered divider state
  // -------------------------------------------------------------------------

  const [hoveredDivider, setHoveredDivider] = useState<string | null>(null)

  // -------------------------------------------------------------------------
  // Reset helpers
  // -------------------------------------------------------------------------

  const handleResetToAuto = useCallback(
    (col: string) => {
      const next = { ...livePinnedWidthsRef.current }
      delete next[col]
      setAndSyncWidths(() => next)
      onOptionsChange({ ...optionsRef.current, columnWidths: next })
    },
    [onOptionsChange, setAndSyncWidths],
  )

  const handleResetAll = useCallback(() => {
    setAndSyncWidths(() => ({}))
    onOptionsChange({ ...optionsRef.current, columnWidths: {} })
  }, [onOptionsChange, setAndSyncWidths])

  // -------------------------------------------------------------------------
  // Effective widths
  // -------------------------------------------------------------------------

  const effectiveWidths = useMemo(() => {
    const result: Record<string, number> = {}
    for (const col of columns) {
      result[col.name] =
        livePinnedWidths[col.name] ?? autoWidths[col.name] ?? MIN_COL_WIDTH_PX
    }
    return result
  }, [columns, livePinnedWidths, autoWidths])

  const hasPinnedWidths = Object.keys(livePinnedWidths).length > 0

  // -------------------------------------------------------------------------
  // Render
  // -------------------------------------------------------------------------

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
              {columns.map((col, colIdx) => {
                const isLast = colIdx === columns.length - 1
                return (
                  <React.Fragment key={col.name}>
                    {renderLogColumn(col, row, {
                      width: effectiveWidths[col.name],
                      isLast,
                    })}
                    {!isLast && (
                      <LogDivider
                        col={col.name}
                        pinned={col.name in livePinnedWidths}
                        hovered={hoveredDivider === col.name}
                        onMouseDown={(e) => handleDividerMouseDown(col.name, e)}
                        onContextMenu={(e) => e.stopPropagation()}
                        onMouseEnter={() => setHoveredDivider(col.name)}
                        onMouseLeave={() => setHoveredDivider(null)}
                        onResetToAuto={() => handleResetToAuto(col.name)}
                        onResetAll={handleResetAll}
                      />
                    )}
                  </React.Fragment>
                )
              })}
            </div>
          )
        })}
      </div>
      <div className="flex justify-between items-center flex-shrink-0">
        <PaginationBar pagination={pagination} />
        {hasPinnedWidths && (
          <button
            onClick={handleResetAll}
            className="text-[10px] px-2 py-0.5 text-theme-text-muted hover:text-theme-text-secondary transition-colors"
          >
            Reset widths
          </button>
        )}
      </div>
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function LogCellEditor({ config, onChange, variables, timeRange, onRun, cellResults, cellSelections }: CellEditorProps) {
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
          minHeight="240px"
          onRunShortcut={onRun}
        />
      </div>
      <AvailableVariablesPanel variables={variables} timeRange={timeRange} cellResults={cellResults} cellSelections={cellSelections} />
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
  icon: <ScrollText />,
  description: 'Log entries viewer with levels',
  showTypeBadge: true,
  defaultHeight: 300,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'log' as const,
    sql: DEFAULT_SQL.log,
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
