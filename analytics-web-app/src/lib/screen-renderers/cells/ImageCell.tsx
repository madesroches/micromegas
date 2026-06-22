import { useState, useEffect, useCallback, useMemo } from 'react'
import { Table } from 'apache-arrow'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { CellConfig, CellState, ImageCellConfig } from '../notebook-types'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { DocumentationLink, QUERY_GUIDE_URL } from '@/components/DocumentationLink'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { substituteMacros } from '../notebook-utils'
import { formatTimestamp } from '@/lib/time-range'
import { Image, ChevronsLeft, ChevronLeft, ChevronRight, ChevronsRight } from 'lucide-react'

const REQUIRED_COLUMNS = ['time', 'name', 'format', 'data'] as const

const DEFAULT_IMAGE_SQL = `SELECT time, name, format, data
FROM view_instance('images', '$process_id')
ORDER BY time`

function validateSchema(table: Table): string | null {
  const fieldNames = new Set(table.schema.fields.map((f) => f.name))
  const missing = REQUIRED_COLUMNS.filter((col) => !fieldNames.has(col))
  if (missing.length > 0) {
    return `Missing required columns: ${missing.join(', ')}`
  }
  return null
}

// =============================================================================
// Renderer Component
// =============================================================================

export function ImageCell({ data, status }: CellRendererProps) {
  const table = data[0]
  const numRows = table?.numRows ?? 0

  const [currentIndex, setCurrentIndex] = useState(0)

  useEffect(() => {
    setCurrentIndex(0)
  }, [table])

  const schemaError = useMemo(() => {
    if (!table || numRows === 0) return null
    return validateSchema(table)
  }, [table, numRows])

  const [blobUrl, setBlobUrl] = useState<string | null>(null)
  const [rowError, setRowError] = useState<string | null>(null)

  useEffect(() => {
    setBlobUrl(null)
    setRowError(null)

    if (!table || numRows === 0 || schemaError) return

    const row = table.get(currentIndex)
    if (!row) return

    const fmt = String(row['format'] ?? '')
    if (!fmt) {
      setRowError('format is null or empty — cannot determine image MIME type')
      return
    }

    const bytes = row['data']
    if (!bytes) {
      setRowError('data is null')
      return
    }

    const blob = new Blob([new Uint8Array(bytes as Uint8Array)], { type: `image/${fmt}` })
    const url = URL.createObjectURL(blob)
    setBlobUrl(url)

    return () => {
      URL.revokeObjectURL(url)
    }
  }, [table, currentIndex, numRows, schemaError])

  const navigate = useCallback(
    (dir: 'first' | 'prev' | 'next' | 'last') => {
      setCurrentIndex((prev) => {
        switch (dir) {
          case 'first': return 0
          case 'prev':  return Math.max(0, prev - 1)
          case 'next':  return Math.min(numRows - 1, prev + 1)
          case 'last':  return numRows - 1
        }
      })
    },
    [numRows],
  )

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center h-full py-8">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
      </div>
    )
  }

  if (numRows === 0) {
    return (
      <div className="flex items-center justify-center h-full py-8 text-theme-text-muted text-sm">
        No images found
      </div>
    )
  }

  if (schemaError) {
    return (
      <div className="flex items-center justify-center h-full py-8 px-4 text-center text-red-400 text-sm">
        {schemaError}
      </div>
    )
  }

  const row = table!.get(currentIndex)
  const currentName = row ? String(row['name'] ?? '') : ''
  const currentTime = row ? formatTimestamp(row['time']) : ''
  const currentFormat = row ? String(row['format'] ?? '') : ''

  const isFirst = currentIndex === 0
  const isLast = currentIndex === numRows - 1

  return (
    <div className="flex flex-col h-full">
      <div className="flex-1 flex items-center justify-center overflow-hidden min-h-0 bg-black/10">
        {rowError ? (
          <span className="text-red-400 text-sm px-4 text-center">{rowError}</span>
        ) : blobUrl ? (
          <img
            src={blobUrl}
            alt={currentName}
            className="max-w-full max-h-full object-contain"
          />
        ) : null}
      </div>

      <div
        className="grid grid-cols-[1fr_auto_1fr] items-center py-0.5 px-1 flex-shrink-0 border-t border-theme-border/40"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 min-w-0 overflow-hidden">
          <span className="text-[10px] text-theme-text-secondary font-medium truncate">
            {currentName}
          </span>
          <span className="text-[10px] text-theme-text-muted whitespace-nowrap shrink-0">
            {currentTime}
          </span>
        </div>

        <div className="flex items-center gap-0.5">
          <NavBtn onClick={() => navigate('first')} disabled={isFirst} title="First image">
            <ChevronsLeft className="w-3 h-3" />
          </NavBtn>
          <NavBtn onClick={() => navigate('prev')} disabled={isFirst} title="Previous image">
            <ChevronLeft className="w-3 h-3" />
          </NavBtn>
          <span className="text-[10px] text-theme-text-muted mx-1 whitespace-nowrap select-none">
            <span className="text-theme-text-secondary font-medium">{currentIndex + 1}</span>
            {' / '}
            <span className="text-theme-text-secondary font-medium">{numRows}</span>
          </span>
          <NavBtn onClick={() => navigate('next')} disabled={isLast} title="Next image">
            <ChevronRight className="w-3 h-3" />
          </NavBtn>
          <NavBtn onClick={() => navigate('last')} disabled={isLast} title="Last image">
            <ChevronsRight className="w-3 h-3" />
          </NavBtn>
        </div>

        <div className="flex items-center justify-end">
          <span className="text-[10px] text-theme-text-muted">{currentFormat}</span>
        </div>
      </div>
    </div>
  )
}

function NavBtn({
  onClick,
  disabled,
  title,
  children,
}: {
  onClick: () => void
  disabled: boolean
  title: string
  children: React.ReactNode
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      title={title}
      className="w-[18px] h-[18px] inline-flex items-center justify-center rounded-sm text-theme-text-muted transition-colors hover:text-theme-text-primary hover:bg-theme-border/40 disabled:opacity-25 disabled:cursor-default disabled:hover:bg-transparent disabled:hover:text-theme-text-muted"
    >
      {children}
    </button>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function ImageCellEditor({
  config,
  onChange,
  variables,
  timeRange,
  onRun,
  cellResults,
  cellSelections,
}: CellEditorProps) {
  const imageConfig = config as ImageCellConfig

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={imageConfig.sql}
          onChange={(sql) => onChange({ ...imageConfig, sql })}
          language="sql"
          placeholder="SELECT time, name, format, data FROM view_instance('images', '$process_id') ORDER BY time"
          minHeight="240px"
          onRunShortcut={onRun}
        />
      </div>
      <AvailableVariablesPanel
        variables={variables}
        timeRange={timeRange}
        cellResults={cellResults}
        cellSelections={cellSelections}
      />
      <DocumentationLink url={QUERY_GUIDE_URL} label="Query Guide" />
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

// eslint-disable-next-line react-refresh/only-export-components
export const imageMetadata: CellTypeMetadata = {
  renderer: ImageCell,
  EditorComponent: ImageCellEditor,

  label: 'Image',
  icon: <Image />,
  description: 'Screenshot carousel from image stream',
  showTypeBadge: true,
  defaultHeight: 500,

  canBlockDownstream: false,

  createDefaultConfig: () => ({
    type: 'image' as const,
    sql: DEFAULT_IMAGE_SQL,
  }),

  execute: async (
    config: CellConfig,
    { variables, cellResults, cellSelections, timeRange, runQuery }: CellExecutionContext,
  ) => {
    const sql = substituteMacros(
      (config as ImageCellConfig).sql,
      variables,
      timeRange,
      cellResults,
      cellSelections,
    )
    const data = await runQuery(sql)
    return { data: [data] }
  },

  getRendererProps: (_config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
  }),
}
