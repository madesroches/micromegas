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
import { timestampToMs } from '@/lib/arrow-utils'

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
}

interface Lane {
  id: string
  name: string
  segments: Segment[]
}

// =============================================================================
// Data Transformation
// =============================================================================

/** Extract lanes and segments from Arrow table */
function extractLanesFromTable(table: Table): Lane[] {
  const laneMap = new Map<string, Lane>()
  const laneOrder: string[] = []

  const idCol = table.getChild('id')
  const nameCol = table.getChild('name')
  const beginCol = table.getChild('begin')
  const endCol = table.getChild('end')

  if (!idCol || !nameCol || !beginCol || !endCol) {
    return []
  }

  const beginField = table.schema.fields.find((f) => f.name === 'begin')
  const endField = table.schema.fields.find((f) => f.name === 'end')

  for (let i = 0; i < table.numRows; i++) {
    const id = String(idCol.get(i) ?? '')
    const name = String(nameCol.get(i) ?? '')
    const begin = timestampToMs(beginCol.get(i), beginField?.type)
    const end = timestampToMs(endCol.get(i), endField?.type)

    if (!id || begin === 0 || end === 0) continue

    if (!laneMap.has(id)) {
      laneMap.set(id, { id, name, segments: [] })
      laneOrder.push(id)
    }

    laneMap.get(id)!.segments.push({ begin, end })
  }

  // Return lanes in first-occurrence order
  return laneOrder.map((id) => laneMap.get(id)!)
}

// =============================================================================
// Time Axis Component
// =============================================================================

function TimeAxis({ from, to }: { from: number; to: number }) {
  const ticks = useMemo(() => {
    const count = 5
    const step = (to - from) / (count - 1)
    return Array.from({ length: count }, (_, i) => from + i * step)
  }, [from, to])

  return (
    <div className="relative h-full">
      {ticks.map((time, i) => {
        const percent = ((time - from) / (to - from)) * 100
        return (
          <span
            key={i}
            className="absolute -translate-x-1/2"
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

function Swimlane({ lanes, timeRange, onTimeRangeSelect }: SwimlaneProps) {
  const duration = timeRange.to - timeRange.from
  const [selection, setSelection] = useState<{ startX: number; currentX: number } | null>(null)
  const [isDragging, setIsDragging] = useState(false)

  // Calculate position as percentage
  const toPercent = (time: number) => {
    return ((time - timeRange.from) / duration) * 100
  }

  // Clamp values to visible range
  const clamp = (value: number, min: number, max: number) => {
    return Math.max(min, Math.min(max, value))
  }

  // Convert pixel position to time
  const pixelToTime = useCallback(
    (pixelX: number, containerWidth: number): number => {
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
    <div className="h-full flex flex-col">
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
                      className="absolute top-1 bottom-1 bg-chart-line rounded-sm opacity-80 hover:opacity-100 transition-opacity pointer-events-none"
                      style={{
                        left: `${startPercent}%`,
                        width: `${Math.max(widthPercent, 0.5)}%`, // Min width for visibility
                      }}
                      title={`${new Date(segment.begin).toLocaleTimeString()} - ${new Date(segment.end).toLocaleTimeString()}`}
                    />
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
  // Convert ISO time range to milliseconds
  const timeRangeMs = useMemo(
    () => ({
      begin: new Date(timeRange.begin).getTime(),
      end: new Date(timeRange.end).getTime(),
    }),
    [timeRange.begin, timeRange.end]
  )

  // Extract lanes from data
  const lanes = useMemo(() => {
    if (!data || data.numRows === 0) {
      return []
    }
    return extractLanesFromTable(data)
  }, [data])

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center h-[200px]">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
      </div>
    )
  }

  if (!data || data.numRows === 0) {
    return (
      <div className="flex items-center justify-center h-[200px] text-theme-text-muted text-sm">
        No data available
      </div>
    )
  }

  return (
    <div className="h-full">
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

function SwimlaneCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const slConfig = config as QueryCellConfig

  // Validate macro references in SQL
  const validationErrors = useMemo(() => {
    return validateMacros(slConfig.sql, variables).errors
  }, [slConfig.sql, variables])

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
          placeholder="SELECT id, name, begin, end FROM ..."
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
      <AvailableVariablesPanel variables={variables} timeRange={timeRange} />
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
  icon: 'S',
  description: 'Horizontal lanes with time segments',
  showTypeBadge: true,
  defaultHeight: 300,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'swimlane' as const,
    sql: DEFAULT_SQL.swimlane,
    options: {},
  }),

  execute: async (config: CellConfig, { variables, timeRange, runQuery }: CellExecutionContext) => {
    const sql = substituteMacros((config as QueryCellConfig).sql, variables, timeRange)
    const data = await runQuery(sql)
    return { data }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
    options: (config as QueryCellConfig).options,
  }),
}
