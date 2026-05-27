/**
 * Pure data model + geometry/format helpers for the flame graph.
 *
 * Extracted from FlameGraphCell.tsx (#1089): no React, no THREE — just Arrow
 * `Table` in, plain numbers/strings out. This is the unit-testable core that
 * `FlameGraphScene` (rendering) and `FlameGraphCell` (the React shell) consume.
 */
import { DataType, Table } from 'apache-arrow'
import { timestampToMs } from '@/lib/arrow-utils'
import { computeAsyncVisualDepths, type SpanData } from './FlameGraphLayout'

// X-axis mode: 'time' uses timestamp columns; 'bits' uses numeric (Int64/Float64)
// columns where each unit represents bits on the wire (see `net_spans` view).
export type XAxisMode = 'time' | 'bits'

export function axisValue(raw: unknown, dataType: DataType, mode: XAxisMode): number {
  if (mode === 'bits') {
    if (raw == null) return NaN
    if (typeof raw === 'number') return raw
    if (typeof raw === 'bigint') return Number(raw)
    const n = Number(raw)
    return Number.isFinite(n) ? n : NaN
  }
  return timestampToMs(raw, dataType)
}

export function detectXAxisMode(dataType: DataType): XAxisMode {
  if (DataType.isTimestamp(dataType)) return 'time'
  return 'bits'
}

// =============================================================================
// Constants
// =============================================================================

export const SPAN_HEIGHT = 20
export const SPAN_GAP = 1
export const LANE_HEADER_HEIGHT = 24
export const LANE_PADDING = 4
export const TIME_AXIS_HEIGHT = 24
export const LABEL_MIN_WIDTH_PX = 40

// =============================================================================
// Brand-derived tricolor palette
// =============================================================================

export const FLAME_PALETTE = [
  // Rust family
  '#8d3a14', '#a33c10', '#bf360c', '#c94e1a', '#d46628',
  // Blue family
  '#0d47a1', '#1565c0', '#1976d2', '#1e88e5', '#2196f3',
  // Gold family
  '#e6a000', '#ecae1a', '#ffb300', '#ffc107', '#ffd54f',
]

export const BLUE_INDICES = new Set([5, 6, 7, 8, 9])

export function spanColorIndex(name: string): number {
  let hash = 0
  for (let i = 0; i < name.length; i++) {
    hash = ((hash << 5) - hash + name.charCodeAt(i)) | 0
  }
  return Math.abs(hash) % FLAME_PALETTE.length
}

export function spanColor(name: string): [hex: string, textLight: boolean] {
  const idx = spanColorIndex(name)
  return [FLAME_PALETTE[idx], BLUE_INDICES.has(idx)]
}

// =============================================================================
// Data Model — FlameIndex
// =============================================================================

export interface LaneIndex {
  id: string
  name: string
  maxDepth: number
  /** Row indices into the Arrow table belonging to this lane, sorted by begin time */
  rowIndices: Int32Array
  /** Visual depth for each entry in rowIndices (greedy-packed to avoid overlap) */
  visualDepths: Int32Array
}

export interface FlameIndex {
  table: Table
  lanes: LaneIndex[]
  timeRange: { min: number; max: number }
  /** Pre-built id → name lookup for O(1) parent name resolution in tooltips */
  idToName: Map<bigint, string>
  /** Whether the X axis represents wall-clock time or bits on the wire. */
  xAxisMode: XAxisMode
  error?: string
}

const REQUIRED_COLUMNS = ['id', 'parent', 'name', 'begin', 'end', 'depth'] as const

export function buildFlameIndex(table: Table): FlameIndex {
  // Validate required columns
  const missingColumns = REQUIRED_COLUMNS.filter((col) => !table.getChild(col))
  if (missingColumns.length > 0) {
    const available = table.schema.fields.map((f) => f.name).join(', ') || 'none'
    return {
      table,
      lanes: [],
      timeRange: { min: 0, max: 0 },
      idToName: new Map(),
      xAxisMode: 'time',
      error: `Missing required columns: ${missingColumns.join(', ')}. Query must return: name, begin, end, depth. Available: ${available}`,
    }
  }

  const beginCol = table.getChild('begin')!
  const endCol = table.getChild('end')!
  const depthCol = table.getChild('depth')!
  const laneCol = table.getChild('lane')

  const beginType = beginCol.type
  const endType = endCol.type
  const xAxisMode = detectXAxisMode(beginType)

  // Single pass: bucket by lane, track min/max time and max depth per lane
  const laneMap = new Map<string, { rows: number[]; maxDepth: number }>()
  const laneOrder: string[] = []
  let globalMin = Infinity
  let globalMax = -Infinity

  for (let i = 0; i < table.numRows; i++) {
    const beginRaw = beginCol.get(i)
    const endRaw = endCol.get(i)
    if (beginRaw == null || endRaw == null) continue

    const begin = axisValue(beginRaw, beginType, xAxisMode)
    const end = axisValue(endRaw, endType, xAxisMode)
    if (isNaN(begin) || isNaN(end)) continue

    const depth = Number(depthCol.get(i) ?? 0)
    const laneName = laneCol ? String(laneCol.get(i) ?? 'default') : 'default'

    if (!laneMap.has(laneName)) {
      laneMap.set(laneName, { rows: [], maxDepth: 0 })
      laneOrder.push(laneName)
    }
    const lane = laneMap.get(laneName)!
    lane.rows.push(i)
    if (depth > lane.maxDepth) lane.maxDepth = depth

    if (begin < globalMin) globalMin = begin
    if (end > globalMax) globalMax = end
  }

  // Sort each lane's rows by begin time, compute visual depths per lane
  const lanes: LaneIndex[] = laneOrder.map((name) => {
    const lane = laneMap.get(name)!
    const rowIndices = new Int32Array(lane.rows)
    rowIndices.sort((a, b) => {
      const aBegin = axisValue(beginCol.get(a), beginType, xAxisMode)
      const bBegin = axisValue(beginCol.get(b), beginType, xAxisMode)
      return aBegin - bBegin
    })

    const isAsync = name === 'async'

    if (isAsync) {
      // Async spans: DFS tree layout via extracted function.
      // Convert Arrow rows to SpanData[], compute layout, map back.
      const idCol = table.getChild('id')!
      const parentCol = table.getChild('parent')!

      const spanDataArr: SpanData[] = []
      for (let i = 0; i < rowIndices.length; i++) {
        const row = rowIndices[i]
        spanDataArr.push({
          id: Number(idCol.get(row) ?? 0),
          parent: Number(parentCol.get(row) ?? 0),
          begin: axisValue(beginCol.get(row), beginType, xAxisMode),
          end: axisValue(endCol.get(row), endType, xAxisMode),
          depth: Number(depthCol.get(row) ?? 0),
        })
      }

      const vdArr = computeAsyncVisualDepths(spanDataArr)
      const visualDepths = new Int32Array(vdArr)
      let maxVisualDepth = 0
      for (const vd of vdArr) {
        if (vd > maxVisualDepth) maxVisualDepth = vd
      }

      return { id: name, name, maxDepth: maxVisualDepth, rowIndices, visualDepths }
    } else {
      // Thread spans: depth from data is the call-stack depth, use directly
      const visualDepths = new Int32Array(rowIndices.length)
      let maxDepth = 0
      for (let i = 0; i < rowIndices.length; i++) {
        const d = Number(depthCol.get(rowIndices[i]) ?? 0)
        visualDepths[i] = d
        if (d > maxDepth) maxDepth = d
      }
      return { id: name, name, maxDepth, rowIndices, visualDepths }
    }
  })

  // Build id → name map for O(1) parent name resolution in tooltips
  const idToName = new Map<bigint, string>()
  const idCol = table.getChild('id')
  const nameCol = table.getChild('name')
  if (idCol && nameCol) {
    for (let i = 0; i < table.numRows; i++) {
      const id = idCol.get(i)
      if (id != null) idToName.set(id, String(nameCol.get(i) ?? ''))
    }
  }

  return {
    table,
    lanes,
    timeRange: { min: globalMin === Infinity ? 0 : globalMin, max: globalMax === -Infinity ? 0 : globalMax },
    idToName,
    xAxisMode,
  }
}

// =============================================================================
// Layout helpers
// =============================================================================

export function laneYOffset(lanes: LaneIndex[], laneIdx: number): number {
  let y = 0
  for (let i = 0; i < laneIdx; i++) {
    y += LANE_HEADER_HEIGHT + (lanes[i].maxDepth + 1) * (SPAN_HEIGHT + SPAN_GAP) + LANE_PADDING
  }
  return y
}

export function totalHeight(lanes: LaneIndex[]): number {
  if (lanes.length === 0) return 0
  return laneYOffset(lanes, lanes.length - 1) +
    LANE_HEADER_HEIGHT + (lanes[lanes.length - 1].maxDepth + 1) * (SPAN_HEIGHT + SPAN_GAP) + LANE_PADDING
}

// =============================================================================
// Hit Testing
// =============================================================================

export interface HitResult {
  rowIndex: number
  laneName: string
}

export function hitTest(
  index: FlameIndex,
  dataX: number, // time in ms or bits (depends on xAxisMode)
  dataY: number, // vertical pixel offset (from top of content)
): HitResult | null {
  const beginCol = index.table.getChild('begin')!
  const endCol = index.table.getChild('end')!
  const beginType = beginCol.type
  const endType = endCol.type

  // Find which lane the Y falls in
  let yAccum = 0
  for (let li = 0; li < index.lanes.length; li++) {
    const lane = index.lanes[li]
    const laneTop = yAccum + LANE_HEADER_HEIGHT
    const laneContentHeight = (lane.maxDepth + 1) * (SPAN_HEIGHT + SPAN_GAP)
    const laneBottom = yAccum + LANE_HEADER_HEIGHT + laneContentHeight + LANE_PADDING

    if (dataY >= laneTop && dataY < laneBottom) {
      // Determine depth band
      const relY = dataY - laneTop
      const depth = Math.floor(relY / (SPAN_HEIGHT + SPAN_GAP))
      if (depth > lane.maxDepth) return null

      for (let i = 0; i < lane.rowIndices.length; i++) {
        const row = lane.rowIndices[i]
        const begin = axisValue(beginCol.get(row), beginType, index.xAxisMode)
        if (begin > dataX) break // past cursor — no more candidates
        const end = axisValue(endCol.get(row), endType, index.xAxisMode)
        if (lane.visualDepths[i] === depth && dataX >= begin && dataX <= end) {
          return { rowIndex: row, laneName: lane.name }
        }
      }
      return null
    }
    yAccum = laneBottom
  }
  return null
}

// =============================================================================
// Format helpers
// =============================================================================

export function formatDuration(ms: number): string {
  if (ms < 0.001) return `${(ms * 1_000_000).toFixed(0)}ns`
  if (ms < 1) return `${(ms * 1000).toFixed(0)}us`
  if (ms < 1000) return `${ms.toFixed(1)}ms`
  return `${(ms / 1000).toFixed(2)}s`
}

export function formatBits(n: number): string {
  const abs = Math.abs(n)
  if (abs < 1_000) return `${n.toFixed(0)} b`
  if (abs < 1_000_000) return `${(n / 1_000).toFixed(1)} Kb`
  if (abs < 1_000_000_000) return `${(n / 1_000_000).toFixed(1)} Mb`
  return `${(n / 1_000_000_000).toFixed(1)} Gb`
}

const TIME_AXIS_FORMAT = new Intl.DateTimeFormat(undefined, {
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
  fractionalSecondDigits: 3,
  hour12: false,
} as Intl.DateTimeFormatOptions)

export function formatAxisTick(value: number, mode: XAxisMode): string {
  return mode === 'bits' ? formatBits(value) : TIME_AXIS_FORMAT.format(value)
}
