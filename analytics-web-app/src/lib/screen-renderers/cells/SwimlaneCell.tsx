import { useState, useCallback, useMemo } from 'react'
import { Table } from 'apache-arrow'
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
import { substituteMacros, validateMacros, DEFAULT_SQL } from '../notebook-utils'
import { timestampToMs, isIntegerType, isStringType, isBinaryType, unwrapDictionary } from '@/lib/arrow-utils'
import { cellColorToCss } from '@/lib/color-utils'
import { AlignCenter } from 'lucide-react'

// =============================================================================
// Constants
// =============================================================================

const LABEL_WIDTH = 128 // w-32 in Tailwind

const TIME_AXIS_FORMAT = new Intl.DateTimeFormat(undefined, {
  hour: '2-digit',
  minute: '2-digit',
  hour12: false,
})

// =============================================================================
// Types
// =============================================================================

interface Segment {
  begin: number
  end: number
  label?: string
  color?: string
}

interface Lane {
  id: string
  name: string
  segments: Segment[]
}

// =============================================================================
// Data Transformation
// =============================================================================

const REQUIRED_COLUMNS = ['id', 'name', 'begin', 'end'] as const

interface ExtractResult {
  lanes: Lane[]
  error?: string
}

// eslint-disable-next-line react-refresh/only-export-components
export function extractLanesFromTable(table: Table): ExtractResult {
  const laneMap = new Map<string, Lane>()
  const laneOrder: string[] = []

  // Check for required columns
  const missingColumns = REQUIRED_COLUMNS.filter((col) => !table.getChild(col))
  if (missingColumns.length > 0) {
    const available = table.schema.fields.map((f) => f.name).join(', ') || 'none'
    return {
      lanes: [],
      error: `Missing required columns: ${missingColumns.join(', ')}. Query must return: id, name, begin, end (label is optional). Available: ${available}`,
    }
  }

  const idCol = table.getChild('id')!
  const nameCol = table.getChild('name')!
  const beginCol = table.getChild('begin')!
  const endCol = table.getChild('end')!
  const labelCol = table.getChild('label') ?? null

  const beginField = table.schema.fields.find((f) => f.name === 'begin')
  const endField = table.schema.fields.find((f) => f.name === 'end')

  const colorCol = table.getChild('color') ?? null
  let colorColumnKind: 'integer' | 'string' | 'binary' | null = null
  if (colorCol) {
    const colorField = table.schema.fields.find((f) => f.name === 'color')!
    const innerType = unwrapDictionary(colorField.type)
    if (isIntegerType(innerType)) {
      colorColumnKind = 'integer'
    } else if (isStringType(innerType)) {
      colorColumnKind = 'string'
    } else if (isBinaryType(colorField.type)) {
      colorColumnKind = 'binary'
    } else {
      return {
        lanes: [],
        error: `'color' column must be integer (packed RGBA u32), string ('#rrggbb'/'#rrggbbaa'), or binary, got ${colorField.type.toString()}`,
      }
    }
  }

  for (let i = 0; i < table.numRows; i++) {
    const id = String(idCol.get(i) ?? '')
    const name = String(nameCol.get(i) ?? '')
    const beginRaw = beginCol.get(i)
    const endRaw = endCol.get(i)

    // Skip rows with missing id or null timestamps
    if (!id || beginRaw == null || endRaw == null) continue

    const begin = timestampToMs(beginRaw, beginField?.type)
    const end = timestampToMs(endRaw, endField?.type)

    // Skip rows with invalid timestamps
    if (isNaN(begin) || isNaN(end)) continue

    if (!laneMap.has(id)) {
      laneMap.set(id, { id, name, segments: [] })
      laneOrder.push(id)
    }

    const labelRaw = labelCol?.get(i)
    const label = labelRaw != null ? String(labelRaw) : undefined

    let color: string | undefined
    if (colorCol && colorColumnKind) {
      const cssColor = cellColorToCss(colorCol.get(i), colorColumnKind)
      if (cssColor != null) color = cssColor
    }

    laneMap.get(id)!.segments.push({ begin, end, label, color })
  }

  // Return lanes in first-occurrence order
  return { lanes: laneOrder.map((id) => laneMap.get(id)!) }
}

// =============================================================================
// Time Axis Component
// =============================================================================

function TimeAxis({ from, to }: { from: number; to: number }) {
  const ticks = useMemo(() => {
    const count = 5
    const range = to - from
    if (range === 0) {
      return [from]
    }
    const step = range / (count - 1)
    return Array.from({ length: count }, (_, i) => from + i * step)
  }, [from, to])

  const range = to - from

  return (
    <div className="relative h-full">
      {ticks.map((time, i) => {
        const percent = range === 0 ? 50 : ((time - from) / range) * 100
        const isFirst = i === 0
        const isLast = i === ticks.length - 1
        return (
          <span
            key={i}
            className={`absolute ${isFirst ? '' : isLast ? '-translate-x-full' : '-translate-x-1/2'}`}
            style={{ left: `${percent}%` }}
          >
            {TIME_AXIS_FORMAT.format(time)}
          </span>
        )
      })}
    </div>
  )
}

// =============================================================================
// Swimlane Component
// =============================================================================

interface SwimlaneProps {
  lanes: Lane[]
  timeRange: { from: number; to: number }
  onTimeRangeSelect?: (from: Date, to: Date) => void
}

const TIME_FORMAT = new Intl.DateTimeFormat(undefined, {
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
  hour12: false,
})

function Swimlane({ lanes, timeRange, onTimeRangeSelect }: SwimlaneProps) {
  const duration = timeRange.to - timeRange.from
  const [selection, setSelection] = useState<{ startX: number; currentX: number } | null>(null)
  const [isDragging, setIsDragging] = useState(false)
  const [tooltip, setTooltip] = useState<{
    x: number
    y: number
    laneName: string
    label: string
    begin: number
    end: number
  } | null>(null)

  // Calculate position as percentage
  const toPercent = (time: number) => {
    if (duration === 0) return 50
    return ((time - timeRange.from) / duration) * 100
  }

  // Clamp values to visible range
  const clamp = (value: number, min: number, max: number) => {
    return Math.max(min, Math.min(max, value))
  }

  // Convert pixel position to time
  const pixelToTime = useCallback(
    (pixelX: number, containerWidth: number): number => {
      if (containerWidth === 0) return timeRange.from
      const ratio = pixelX / containerWidth
      return timeRange.from + ratio * duration
    },
    [timeRange.from, duration]
  )

  // Get pixel position relative to the timeline bar
  const getRelativeX = useCallback((e: React.MouseEvent, element: HTMLElement): number => {
    const rect = element.getBoundingClientRect()
    return Math.max(0, Math.min(e.clientX - rect.left, rect.width))
  }, [])

  const handleMouseDown = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!onTimeRangeSelect) return
      const target = e.currentTarget
      const x = getRelativeX(e, target)
      setSelection({ startX: x, currentX: x })
      setIsDragging(true)
      e.preventDefault()
    },
    [onTimeRangeSelect, getRelativeX]
  )

  const handleMouseMove = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!isDragging || !selection) return
      const target = e.currentTarget
      const x = getRelativeX(e, target)
      setSelection((prev) => (prev ? { ...prev, currentX: x } : null))
    },
    [isDragging, selection, getRelativeX]
  )

  const handleMouseUp = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!isDragging || !selection || !onTimeRangeSelect) {
        setSelection(null)
        setIsDragging(false)
        return
      }

      const target = e.currentTarget
      const containerWidth = target.getBoundingClientRect().width
      const minX = Math.min(selection.startX, selection.currentX)
      const maxX = Math.max(selection.startX, selection.currentX)

      // Only trigger if selection is meaningful (at least 5 pixels)
      if (maxX - minX > 5) {
        const fromTime = pixelToTime(minX, containerWidth)
        const toTime = pixelToTime(maxX, containerWidth)
        onTimeRangeSelect(new Date(fromTime), new Date(toTime))
      }

      setSelection(null)
      setIsDragging(false)
    },
    [isDragging, selection, onTimeRangeSelect, pixelToTime]
  )

  const handleMouseLeave = useCallback(() => {
    if (isDragging) {
      setSelection(null)
      setIsDragging(false)
    }
  }, [isDragging])

  // Calculate selection overlay position
  const getSelectionStyle = useCallback(() => {
    if (!selection) return { display: 'none' }
    const left = Math.min(selection.startX, selection.currentX)
    const width = Math.abs(selection.currentX - selection.startX)
    return {
      left: `${left}px`,
      width: `${width}px`,
      display: width > 2 ? 'block' : 'none',
    }
  }, [selection])

  if (lanes.length === 0) {
    return (
      <div className="flex items-center justify-center h-full text-theme-text-muted text-sm">
        No swimlane data available
      </div>
    )
  }

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      {/* Scrollable lanes area */}
      <div className="flex-1 overflow-auto min-h-0">
        <div className="divide-y divide-theme-border/50">
          {lanes.map((lane) => (
            <div key={lane.id} className="flex items-center h-8">
              {/* Lane name */}
              <div
                className="flex-shrink-0 px-2 text-xs font-medium text-theme-text-secondary truncate"
                style={{ width: LABEL_WIDTH }}
                title={lane.name}
              >
                {lane.name}
              </div>

              {/* Timeline bar area */}
              <div
                className={`flex-1 h-6 relative bg-app-bg rounded ${onTimeRangeSelect ? 'cursor-crosshair' : ''}`}
                onMouseDown={handleMouseDown}
                onMouseMove={handleMouseMove}
                onMouseUp={handleMouseUp}
                onMouseLeave={handleMouseLeave}
              >
                {/* Selection overlay */}
                {selection && (
                  <div
                    className="absolute top-0 bottom-0 pointer-events-none z-20"
                    style={{
                      ...getSelectionStyle(),
                      background: 'var(--chart-selection)',
                      borderLeft: '2px solid var(--chart-selection-border)',
                      borderRight: '2px solid var(--chart-selection-border)',
                    }}
                  />
                )}

                {/* Segments */}
                {lane.segments.map((segment, idx) => {
                  const startPercent = clamp(toPercent(segment.begin), 0, 100)
                  const endPercent = clamp(toPercent(segment.end), 0, 100)
                  const widthPercent = endPercent - startPercent

                  // Skip segments entirely outside the visible range
                  if (widthPercent <= 0) return null

                  return (
                    <div
                      key={idx}
                      className="absolute top-1 bottom-1 rounded-sm flex items-center overflow-hidden transition-opacity hover:opacity-85 hover:ring-1 hover:ring-brand-gold"
                      style={{
                        left: `${startPercent}%`,
                        width: `${Math.max(widthPercent, 0.5)}%`,
                        backgroundColor: segment.color ?? 'var(--chart-line)',
                      }}
                      onMouseEnter={(e) => {
                        if (segment.label != null && !isDragging) {
                          setTooltip({
                            x: e.clientX,
                            y: e.clientY,
                            laneName: lane.name,
                            label: segment.label,
                            begin: segment.begin,
                            end: segment.end,
                          })
                        }
                      }}
                      onMouseMove={(e) => {
                        if (segment.label != null && !isDragging) {
                          setTooltip((prev) => prev ? { ...prev, x: e.clientX, y: e.clientY } : null)
                        }
                      }}
                      onMouseLeave={() => setTooltip(null)}
                    >
                      {segment.label != null && (
                        <span className="truncate px-1 text-[10px] font-medium text-white pointer-events-none">
                          {segment.label}
                        </span>
                      )}
                    </div>
                  )
                })}
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* Time axis - fixed at bottom, outside scrollable area */}
      <div className="flex-shrink-0 flex items-center h-6 text-[10px] text-theme-text-muted border-t border-theme-border/30 pt-1">
        <div style={{ width: LABEL_WIDTH }} />
        <div className="flex-1 relative">
          <TimeAxis from={timeRange.from} to={timeRange.to} />
        </div>
      </div>

      {/* Tooltip */}
      {tooltip && (
        <div
          className="fixed bg-app-bg border border-theme-border rounded-md px-3 py-2 text-xs pointer-events-none z-50 shadow-lg"
          style={{
            left: Math.min(tooltip.x + 15, window.innerWidth - 196),
            top: Math.min(tooltip.y - 10, window.innerHeight - 76),
          }}
        >
          <div className="text-theme-text-muted text-[10px] mb-1">{tooltip.laneName}</div>
          <div className="text-theme-text-primary font-medium">{tooltip.label}</div>
          <div className="text-theme-text-secondary text-[10px] mt-1">
            {TIME_FORMAT.format(tooltip.begin)} → {TIME_FORMAT.format(tooltip.end)}
          </div>
        </div>
      )}
    </div>
  )
}

// =============================================================================
// Renderer Component
// =============================================================================

export function SwimlaneCell({
  data,
  status,
  timeRange,
  onTimeRangeSelect,
}: CellRendererProps) {
  const table = data[0]
  // Convert ISO time range to milliseconds
  const timeRangeMs = useMemo(
    () => ({
      begin: new Date(timeRange.begin).getTime(),
      end: new Date(timeRange.end).getTime(),
    }),
    [timeRange.begin, timeRange.end]
  )

  // Extract lanes from data
  const { lanes, error: schemaError } = useMemo(() => {
    if (!table || table.numRows === 0) {
      return { lanes: [] }
    }
    return extractLanesFromTable(table)
  }, [table])

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center h-[200px]">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
      </div>
    )
  }

  if (schemaError) {
    return (
      <div className="flex items-center justify-center h-[200px] text-red-400 text-sm px-4 text-center">
        {schemaError}
      </div>
    )
  }

  if (!table || table.numRows === 0) {
    return (
      <div className="flex items-center justify-center h-[200px] text-theme-text-muted text-sm">
        No data available
      </div>
    )
  }

  return (
    <div className="flex-1 min-h-0">
      <Swimlane
        lanes={lanes}
        timeRange={{ from: timeRangeMs.begin, to: timeRangeMs.end }}
        onTimeRangeSelect={onTimeRangeSelect}
      />
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function SwimlaneCellEditor({ config, onChange, variables, timeRange, onRun, cellResults, cellSelections }: CellEditorProps) {
  const slConfig = config as QueryCellConfig

  // Validate macro references in SQL
  const validationErrors = useMemo(() => {
    return validateMacros(slConfig.sql, variables, cellResults, cellSelections).errors
  }, [slConfig.sql, variables, cellResults, cellSelections])

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={slConfig.sql}
          onChange={(sql) => onChange({ ...slConfig, sql })}
          language="sql"
          placeholder="SELECT id, name, begin, end [, label] [, color] FROM ..."
          minHeight="240px"
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
      <AvailableVariablesPanel variables={variables} timeRange={timeRange} cellResults={cellResults} cellSelections={cellSelections} />
      <DocumentationLink url={QUERY_GUIDE_URL} label="Query Guide" />
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

// eslint-disable-next-line react-refresh/only-export-components
export const swimlaneMetadata: CellTypeMetadata = {
  renderer: SwimlaneCell,
  EditorComponent: SwimlaneCellEditor,

  label: 'Swimlane',
  icon: <AlignCenter />,
  description: 'Horizontal lanes with time segments',
  showTypeBadge: true,
  defaultHeight: 300,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'swimlane' as const,
    sql: DEFAULT_SQL.swimlane,
    options: {},
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
