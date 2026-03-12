import { useRef, useEffect, useCallback, useMemo } from 'react'
import { Table } from 'apache-arrow'
import * as THREE from 'three'
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
import { Flame } from 'lucide-react'

// =============================================================================
// Constants
// =============================================================================

const SPAN_HEIGHT = 20
const SPAN_GAP = 1
const LANE_HEADER_HEIGHT = 24
const LANE_PADDING = 4
const TIME_AXIS_HEIGHT = 24
const LABEL_MIN_WIDTH_PX = 40

// =============================================================================
// Brand-derived tricolor palette
// =============================================================================

const FLAME_PALETTE = [
  // Rust family
  '#8d3a14', '#a33c10', '#bf360c', '#c94e1a', '#d46628',
  // Blue family
  '#0d47a1', '#1565c0', '#1976d2', '#1e88e5', '#2196f3',
  // Gold family
  '#e6a000', '#ecae1a', '#ffb300', '#ffc107', '#ffd54f',
]

const BLUE_INDICES = new Set([5, 6, 7, 8, 9])

function spanColorIndex(name: string): number {
  let hash = 0
  for (let i = 0; i < name.length; i++) {
    hash = ((hash << 5) - hash + name.charCodeAt(i)) | 0
  }
  return Math.abs(hash) % FLAME_PALETTE.length
}

function spanColor(name: string): [hex: string, textLight: boolean] {
  const idx = spanColorIndex(name)
  return [FLAME_PALETTE[idx], BLUE_INDICES.has(idx)]
}

// =============================================================================
// Async span layout (exported for testing)
// =============================================================================

export interface SpanData {
  id: number
  parent: number
  begin: number
  end: number
  depth: number
}

/**
 * Compute visual depths for async spans using DFS tree-walk layout.
 * Children are placed directly below their parent; concurrent siblings
 * get bumped to the next visual row.
 * Returns an array of visual depths, one per input span (in input order).
 */
export function computeAsyncVisualDepths(spans: SpanData[]): number[] {
  const n = spans.length
  if (n === 0) return []

  // Build id → index lookup
  const idToIdx = new Map<number, number>()
  for (let i = 0; i < n; i++) {
    idToIdx.set(spans[i].id, i)
  }

  // Build children map and find roots
  const childrenOf = new Map<number, number[]>()
  const roots: number[] = []
  for (let i = 0; i < n; i++) {
    const parentIdx = idToIdx.get(spans[i].parent)
    if (parentIdx != null && parentIdx !== i) {
      if (!childrenOf.has(spans[i].parent)) childrenOf.set(spans[i].parent, [])
      childrenOf.get(spans[i].parent)!.push(i)
    } else {
      roots.push(i)
    }
  }

  // Collect subtrees
  interface SubTree {
    members: number[]
    minBegin: number
    maxEnd: number
  }
  const visited = new Uint8Array(n)
  const trees: SubTree[] = []

  for (const rootIdx of roots) {
    const members: number[] = []
    let minBegin = Infinity
    let maxEnd = -Infinity
    const stack: number[] = [rootIdx]

    while (stack.length > 0) {
      const idx = stack.pop()!
      if (visited[idx]) continue
      visited[idx] = 1
      members.push(idx)

      if (spans[idx].begin < minBegin) minBegin = spans[idx].begin
      if (spans[idx].end > maxEnd) maxEnd = spans[idx].end

      const children = childrenOf.get(spans[idx].id)
      if (children) {
        for (let c = children.length - 1; c >= 0; c--) {
          stack.push(children[c])
        }
      }
    }

    trees.push({ members, minBegin, maxEnd })
  }

  // Layout each subtree using DFS order + row-end tracking.
  // DFS ensures children are processed right after their parent.
  // Row-end tracking allows non-overlapping siblings to share visual rows.
  const globalRowEnds: number[] = []
  const visualDepths = new Array<number>(n).fill(0)

  for (const tree of trees) {
    const memberRelVd = new Map<number, number>()
    let treeHeight = 0

    // Find roots within this subtree
    const treeRoots = tree.members.filter((idx) => {
      const parentIdx = idToIdx.get(spans[idx].parent)
      return parentIdx == null || parentIdx === idx
    })

    // DFS stack: process each node, then its children immediately after
    const dfsStack: { idx: number; parentVd: number }[] = []
    for (let i = treeRoots.length - 1; i >= 0; i--) {
      dfsStack.push({ idx: treeRoots[i], parentVd: -1 })
    }

    // Track end time per visual row — non-overlapping spans reuse the same row
    const vdRowEnds = new Map<number, number>()

    while (dfsStack.length > 0) {
      const { idx, parentVd } = dfsStack.pop()!
      const s = spans[idx]
      const baseVd = parentVd + 1

      // Find first available visual row at baseVd or deeper
      let vd = baseVd
      for (;;) {
        const endTime = vdRowEnds.get(vd)
        if (endTime == null || s.begin >= endTime) {
          vdRowEnds.set(vd, s.end)
          break
        }
        vd++
      }

      memberRelVd.set(idx, vd)
      if (vd + 1 > treeHeight) treeHeight = vd + 1

      // Push children in reverse-begin order so earliest is popped first
      const children = childrenOf.get(s.id)
      if (children) {
        const sorted = [...children].sort((a, b) => spans[b].begin - spans[a].begin)
        for (const childIdx of sorted) {
          dfsStack.push({ idx: childIdx, parentVd: vd })
        }
      }
    }

    // Find lowest global base where the tree block fits
    let base = 0
    let searching = true
    while (searching) {
      searching = false
      for (let d = 0; d < treeHeight; d++) {
        const r = base + d
        if (r < globalRowEnds.length && globalRowEnds[r] > tree.minBegin) {
          base = r + 1
          searching = true
          break
        }
      }
    }

    // Assign global visual depths
    for (const idx of tree.members) {
      visualDepths[idx] = base + memberRelVd.get(idx)!
    }

    // Reserve the tree block
    for (let d = 0; d < treeHeight; d++) {
      const r = base + d
      while (globalRowEnds.length <= r) globalRowEnds.push(0)
      if (tree.maxEnd > globalRowEnds[r]) globalRowEnds[r] = tree.maxEnd
    }
  }

  return visualDepths
}

// =============================================================================
// Data Model — FlameIndex
// =============================================================================

interface LaneIndex {
  id: string
  name: string
  maxDepth: number
  /** Row indices into the Arrow table belonging to this lane, sorted by begin time */
  rowIndices: Int32Array
  /** Visual depth for each entry in rowIndices (greedy-packed to avoid overlap) */
  visualDepths: Int32Array
}

interface FlameIndex {
  table: Table
  lanes: LaneIndex[]
  timeRange: { min: number; max: number }
  error?: string
}

const REQUIRED_COLUMNS = ['id', 'parent', 'name', 'begin', 'end', 'depth'] as const

function buildFlameIndex(table: Table): FlameIndex {
  // Validate required columns
  const missingColumns = REQUIRED_COLUMNS.filter((col) => !table.getChild(col))
  if (missingColumns.length > 0) {
    const available = table.schema.fields.map((f) => f.name).join(', ') || 'none'
    return {
      table,
      lanes: [],
      timeRange: { min: 0, max: 0 },
      error: `Missing required columns: ${missingColumns.join(', ')}. Query must return: name, begin, end, depth. Available: ${available}`,
    }
  }

  const beginCol = table.getChild('begin')!
  const endCol = table.getChild('end')!
  const depthCol = table.getChild('depth')!
  const laneCol = table.getChild('lane')

  const beginField = table.schema.fields.find((f) => f.name === 'begin')
  const endField = table.schema.fields.find((f) => f.name === 'end')

  // Single pass: bucket by lane, track min/max time and max depth per lane
  const laneMap = new Map<string, { rows: number[]; maxDepth: number }>()
  const laneOrder: string[] = []
  let globalMin = Infinity
  let globalMax = -Infinity

  for (let i = 0; i < table.numRows; i++) {
    const beginRaw = beginCol.get(i)
    const endRaw = endCol.get(i)
    if (beginRaw == null || endRaw == null) continue

    const begin = timestampToMs(beginRaw, beginField?.type)
    const end = timestampToMs(endRaw, endField?.type)
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
      const aBegin = timestampToMs(beginCol.get(a), beginField?.type)
      const bBegin = timestampToMs(beginCol.get(b), beginField?.type)
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
          begin: timestampToMs(beginCol.get(row), beginField?.type),
          end: timestampToMs(endCol.get(row), endField?.type),
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

  return {
    table,
    lanes,
    timeRange: { min: globalMin === Infinity ? 0 : globalMin, max: globalMax === -Infinity ? 0 : globalMax },
  }
}

// =============================================================================
// Layout helpers
// =============================================================================

function laneYOffset(lanes: LaneIndex[], laneIdx: number): number {
  let y = 0
  for (let i = 0; i < laneIdx; i++) {
    y += LANE_HEADER_HEIGHT + (lanes[i].maxDepth + 1) * (SPAN_HEIGHT + SPAN_GAP) + LANE_PADDING
  }
  return y
}

function totalHeight(lanes: LaneIndex[]): number {
  if (lanes.length === 0) return 0
  return laneYOffset(lanes, lanes.length - 1) +
    LANE_HEADER_HEIGHT + (lanes[lanes.length - 1].maxDepth + 1) * (SPAN_HEIGHT + SPAN_GAP) + LANE_PADDING
}

// =============================================================================
// Hit Testing
// =============================================================================

interface HitResult {
  rowIndex: number
  laneName: string
}

function hitTest(
  index: FlameIndex,
  dataX: number, // time in ms
  dataY: number, // vertical pixel offset (from top of content)
): HitResult | null {
  const beginCol = index.table.getChild('begin')!
  const endCol = index.table.getChild('end')!
  const beginField = index.table.schema.fields.find((f) => f.name === 'begin')
  const endField = index.table.schema.fields.find((f) => f.name === 'end')

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
        const begin = timestampToMs(beginCol.get(row), beginField?.type)
        if (begin > dataX) break // past cursor — no more candidates
        const end = timestampToMs(endCol.get(row), endField?.type)
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

function formatDuration(ms: number): string {
  if (ms < 1) return `${(ms * 1000).toFixed(0)}us`
  if (ms < 1000) return `${ms.toFixed(1)}ms`
  return `${(ms / 1000).toFixed(2)}s`
}

const TIME_AXIS_FORMAT = new Intl.DateTimeFormat(undefined, {
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
  fractionalSecondDigits: 3,
  hour12: false,
} as Intl.DateTimeFormatOptions)

// =============================================================================
// FlameGraph Renderer (Three.js + Canvas2D + DOM)
// =============================================================================

interface FlameGraphViewProps {
  index: FlameIndex
  onTimeRangeSelect?: (from: Date, to: Date) => void
}

function FlameGraphView({ index, onTimeRangeSelect }: FlameGraphViewProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const webglCanvasRef = useRef<HTMLCanvasElement>(null)
  const textCanvasRef = useRef<HTMLCanvasElement>(null)
  const tooltipRef = useRef<HTMLDivElement>(null)

  // Rendering state stored in refs to avoid re-renders
  const stateRef = useRef({
    renderer: null as THREE.WebGLRenderer | null,
    camera: null as THREE.OrthographicCamera | null,
    scene: null as THREE.Scene | null,
    mesh: null as THREE.InstancedMesh | null,
    // View state
    viewMinTime: index.timeRange.min,
    viewMaxTime: index.timeRange.max,
    scrollY: 0,
    // Mouse position (for cursor-anchored zoom)
    mouseX: 0,
    // Drag-to-zoom state
    isDragging: false,
    isPanning: false,
    dragStartX: 0,
    dragCurrentX: 0,
    // Dimensions
    width: 0,
    height: 0,
    contentHeight: totalHeight(index.lanes),
    // Allocated instance count
    maxInstances: 0,
  })

  const animFrameRef = useRef(0)

  // -----------------------------------------------------------------------
  // Core render function
  // -----------------------------------------------------------------------
  const render = useCallback(() => {
    const s = stateRef.current
    if (!s.renderer || !s.camera || !s.scene || !s.mesh) return
    if (s.width === 0 || s.height === 0) return

    const canvasHeight = s.height - TIME_AXIS_HEIGHT
    const timeSpan = s.viewMaxTime - s.viewMinTime
    if (timeSpan <= 0) return

    const beginCol = index.table.getChild('begin')!
    const endCol = index.table.getChild('end')!
    const nameCol = index.table.getChild('name')!
    const beginField = index.table.schema.fields.find((f) => f.name === 'begin')
    const endField = index.table.schema.fields.find((f) => f.name === 'end')

    const pxPerMs = s.width / timeSpan
    const mat = new THREE.Matrix4()
    const col = new THREE.Color()
    let instanceIdx = 0

    // Ensure mesh has enough capacity
    const estimatedMax = index.table.numRows
    if (estimatedMax > s.maxInstances) {
      // Recreate mesh with larger capacity
      s.scene.remove(s.mesh)
      s.mesh.dispose()
      const geo = new THREE.PlaneGeometry(1, 1)
      const material = new THREE.MeshBasicMaterial({ color: 0xffffff })
      s.mesh = new THREE.InstancedMesh(geo, material, estimatedMax)
      s.mesh.frustumCulled = false
      s.scene.add(s.mesh)
      s.maxInstances = estimatedMax
    }

    // Populate instances for visible spans
    for (let li = 0; li < index.lanes.length; li++) {
      const lane = index.lanes[li]
      const laneTop = laneYOffset(index.lanes, li) + LANE_HEADER_HEIGHT - s.scrollY

      // Skip lanes entirely off-screen
      const laneContentHeight = (lane.maxDepth + 1) * (SPAN_HEIGHT + SPAN_GAP)
      if (laneTop + laneContentHeight < 0 || laneTop > canvasHeight) continue

      for (let i = 0; i < lane.rowIndices.length; i++) {
        const row = lane.rowIndices[i]
        const begin = timestampToMs(beginCol.get(row), beginField?.type)
        if (begin >= s.viewMaxTime) break // sorted by begin — nothing further can be visible

        const end = timestampToMs(endCol.get(row), endField?.type)
        if (end <= s.viewMinTime) continue // ends before viewport

        const depth = lane.visualDepths[i]
        const name = String(nameCol.get(row) ?? '')

        // Pixel coordinates
        const x1 = (begin - s.viewMinTime) * pxPerMs
        const x2 = (end - s.viewMinTime) * pxPerMs
        const w = Math.max(x2 - x1, 1) // min 1px width
        const y = laneTop + depth * (SPAN_HEIGHT + SPAN_GAP)

        // Skip if off-screen vertically
        if (y + SPAN_HEIGHT < 0 || y > canvasHeight) continue

        // Set instance transform: translate to center, scale to size
        mat.makeScale(w, SPAN_HEIGHT, 1)
        mat.setPosition(x1 + w / 2, canvasHeight - y - SPAN_HEIGHT / 2, 0)
        s.mesh.setMatrixAt(instanceIdx, mat)

        // Color
        const [hex] = spanColor(name)
        col.set(hex)
        s.mesh.setColorAt(instanceIdx, col)

        instanceIdx++
      }
    }

    s.mesh.count = instanceIdx
    s.mesh.instanceMatrix.needsUpdate = true
    if (s.mesh.instanceColor) s.mesh.instanceColor.needsUpdate = true

    // Camera: orthographic pixel-space
    s.camera.left = 0
    s.camera.right = s.width
    s.camera.top = canvasHeight
    s.camera.bottom = 0
    s.camera.near = -1
    s.camera.far = 1
    s.camera.updateProjectionMatrix()

    s.renderer.render(s.scene, s.camera)

    // --- Canvas2D overlay: labels + time axis + selection ---
    const textCanvas = textCanvasRef.current
    if (!textCanvas) return
    const ctx = textCanvas.getContext('2d')
    if (!ctx) return

    const dpr = window.devicePixelRatio || 1
    ctx.clearRect(0, 0, textCanvas.width, textCanvas.height)
    ctx.save()
    ctx.scale(dpr, dpr)

    // Draw span labels
    ctx.font = '11px monospace'
    ctx.textBaseline = 'middle'

    for (let li = 0; li < index.lanes.length; li++) {
      const lane = index.lanes[li]
      const laneTop = laneYOffset(index.lanes, li) + LANE_HEADER_HEIGHT - s.scrollY

      const laneContentHeight = (lane.maxDepth + 1) * (SPAN_HEIGHT + SPAN_GAP)
      if (laneTop + laneContentHeight < 0 || laneTop > canvasHeight) continue

      for (let i = 0; i < lane.rowIndices.length; i++) {
        const row = lane.rowIndices[i]
        const begin = timestampToMs(beginCol.get(row), beginField?.type)
        if (begin > s.viewMaxTime) break

        const end = timestampToMs(endCol.get(row), endField?.type)
        if (end < s.viewMinTime) continue

        const depth = lane.visualDepths[i]
        const name = String(nameCol.get(row) ?? '')

        const x1 = (begin - s.viewMinTime) * pxPerMs
        const x2 = (end - s.viewMinTime) * pxPerMs
        const w = x2 - x1
        const y = laneTop + depth * (SPAN_HEIGHT + SPAN_GAP)

        if (y + SPAN_HEIGHT < 0 || y > canvasHeight) continue
        if (w < LABEL_MIN_WIDTH_PX) continue

        const [, textLight] = spanColor(name)
        ctx.fillStyle = textLight ? '#ffffff' : '#000000'

        ctx.save()
        ctx.beginPath()
        ctx.rect(Math.max(x1 + 2, 0), y, Math.min(w - 4, s.width), SPAN_HEIGHT)
        ctx.clip()
        ctx.fillText(name, x1 + 4, y + SPAN_HEIGHT / 2 + 1)
        ctx.restore()
      }
    }

    // Draw lane headers
    ctx.font = 'bold 11px sans-serif'
    ctx.textBaseline = 'middle'
    ctx.fillStyle = '#9ca3af' // gray-400
    for (let li = 0; li < index.lanes.length; li++) {
      const lane = index.lanes[li]
      const headerY = laneYOffset(index.lanes, li) - s.scrollY
      if (headerY + LANE_HEADER_HEIGHT < 0 || headerY > canvasHeight) continue
      ctx.fillText(lane.name, 4, headerY + LANE_HEADER_HEIGHT / 2)
    }

    // Draw time axis
    const axisY = canvasHeight
    ctx.fillStyle = '#1a1a2e'
    ctx.fillRect(0, axisY, s.width, TIME_AXIS_HEIGHT)

    ctx.font = '10px monospace'
    ctx.fillStyle = '#9ca3af'
    ctx.textBaseline = 'top'
    const tickCount = Math.max(2, Math.floor(s.width / 120))
    const tickStep = timeSpan / (tickCount - 1)
    for (let t = 0; t < tickCount; t++) {
      const time = s.viewMinTime + t * tickStep
      const x = t * (s.width / (tickCount - 1))
      ctx.fillText(TIME_AXIS_FORMAT.format(time), t === tickCount - 1 ? Math.max(0, x - 80) : x + 2, axisY + 4)
      ctx.strokeStyle = '#374151'
      ctx.beginPath()
      ctx.moveTo(x, axisY)
      ctx.lineTo(x, axisY + 4)
      ctx.stroke()
    }

    // Draw selection overlay
    if (s.isDragging && !s.isPanning) {
      const selLeft = Math.min(s.dragStartX, s.dragCurrentX)
      const selWidth = Math.abs(s.dragCurrentX - s.dragStartX)
      if (selWidth > 2) {
        ctx.fillStyle = 'rgba(59, 130, 246, 0.2)'
        ctx.fillRect(selLeft, 0, selWidth, canvasHeight)
        ctx.strokeStyle = 'rgba(59, 130, 246, 0.6)'
        ctx.lineWidth = 2
        ctx.beginPath()
        ctx.moveTo(selLeft, 0)
        ctx.lineTo(selLeft, canvasHeight)
        ctx.moveTo(selLeft + selWidth, 0)
        ctx.lineTo(selLeft + selWidth, canvasHeight)
        ctx.stroke()
      }
    }

    ctx.restore()
  }, [index])

  const requestRender = useCallback(() => {
    cancelAnimationFrame(animFrameRef.current)
    animFrameRef.current = requestAnimationFrame(render)
  }, [render])

  // -----------------------------------------------------------------------
  // Three.js setup / teardown
  // -----------------------------------------------------------------------
  useEffect(() => {
    const webglCanvas = webglCanvasRef.current
    const textCanvas = textCanvasRef.current
    const container = containerRef.current
    if (!webglCanvas || !textCanvas || !container) return

    const dpr = window.devicePixelRatio || 1
    const rect = container.getBoundingClientRect()
    const w = Math.floor(rect.width)
    const h = Math.floor(rect.height)

    const s = stateRef.current
    s.width = w
    s.height = h
    s.viewMinTime = index.timeRange.min
    s.viewMaxTime = index.timeRange.max
    s.contentHeight = totalHeight(index.lanes)
    s.scrollY = 0

    // WebGL canvas
    webglCanvas.width = w * dpr
    webglCanvas.height = (h - TIME_AXIS_HEIGHT) * dpr
    webglCanvas.style.width = `${w}px`
    webglCanvas.style.height = `${h - TIME_AXIS_HEIGHT}px`

    // Text canvas
    textCanvas.width = w * dpr
    textCanvas.height = h * dpr
    textCanvas.style.width = `${w}px`
    textCanvas.style.height = `${h}px`

    // Three.js renderer
    const renderer = new THREE.WebGLRenderer({ canvas: webglCanvas, antialias: false, alpha: true })
    renderer.setPixelRatio(dpr)
    renderer.setSize(w, h - TIME_AXIS_HEIGHT, false)

    const camera = new THREE.OrthographicCamera(0, w, h - TIME_AXIS_HEIGHT, 0, -1, 1)
    const scene = new THREE.Scene()

    const initialCapacity = Math.max(index.table.numRows, 1024)
    const geo = new THREE.PlaneGeometry(1, 1)
    // White base color — instance colors from setColorAt() are multiplied with this.
    // Do NOT use vertexColors:true — PlaneGeometry has no vertex color attribute,
    // which zeroes out the color. InstancedMesh has its own USE_INSTANCING_COLOR path.
    const material = new THREE.MeshBasicMaterial({ color: 0xffffff })
    const mesh = new THREE.InstancedMesh(geo, material, initialCapacity)
    mesh.frustumCulled = false
    scene.add(mesh)

    s.renderer = renderer
    s.camera = camera
    s.scene = scene
    s.mesh = mesh
    s.maxInstances = initialCapacity

    requestRender()

    // Resize observer
    const resizeObserver = new ResizeObserver((entries) => {
      const entry = entries[0]
      if (!entry) return
      const newW = Math.floor(entry.contentRect.width)
      const newH = Math.floor(entry.contentRect.height)
      if (newW === s.width && newH === s.height) return

      s.width = newW
      s.height = newH

      const newDpr = window.devicePixelRatio || 1
      webglCanvas.width = newW * newDpr
      webglCanvas.height = (newH - TIME_AXIS_HEIGHT) * newDpr
      webglCanvas.style.width = `${newW}px`
      webglCanvas.style.height = `${newH - TIME_AXIS_HEIGHT}px`

      textCanvas.width = newW * newDpr
      textCanvas.height = newH * newDpr
      textCanvas.style.width = `${newW}px`
      textCanvas.style.height = `${newH}px`

      renderer.setSize(newW, newH - TIME_AXIS_HEIGHT, false)
      requestRender()
    })
    resizeObserver.observe(container)

    return () => {
      resizeObserver.disconnect()
      cancelAnimationFrame(animFrameRef.current)
      scene.remove(mesh)
      mesh.dispose()
      geo.dispose()
      material.dispose()
      renderer.dispose()
      s.renderer = null
      s.camera = null
      s.scene = null
      s.mesh = null
    }
  }, [index, requestRender])

  // -----------------------------------------------------------------------
  // Interaction: WASD zoom/pan, wheel scroll, drag-to-zoom, hover tooltip
  // -----------------------------------------------------------------------

  // WASD continuous key state
  const keysRef = useRef(new Set<string>())
  const keyAnimRef = useRef(0)

  const keyTick = useCallback(() => {
    const s = stateRef.current
    const keys = keysRef.current
    if (keys.size === 0) { keyAnimRef.current = 0; return }

    const span = s.viewMaxTime - s.viewMinTime
    const fullSpan = index.timeRange.max - index.timeRange.min
    const panStep = span * 0.03 // 3% of visible range per frame
    const zoomFactor = 1.15     // 15% zoom per frame

    if (keys.has('a')) { s.viewMinTime -= panStep; s.viewMaxTime -= panStep }
    if (keys.has('d')) { s.viewMinTime += panStep; s.viewMaxTime += panStep }
    if (keys.has('w') || keys.has('s')) {
      // Cursor-anchored zoom: the time under the mouse stays fixed
      const ratio = s.width > 0 ? s.mouseX / s.width : 0.5
      const cursorTime = s.viewMinTime + ratio * span
      const newSpan = keys.has('w')
        ? Math.max(0.001, span / zoomFactor)
        : Math.min(fullSpan, span * zoomFactor)
      s.viewMinTime = cursorTime - ratio * newSpan
      s.viewMaxTime = cursorTime + (1 - ratio) * newSpan
    }

    requestRender()
    keyAnimRef.current = requestAnimationFrame(keyTick)
  }, [index, requestRender])

  const handleWheel = useCallback(
    (e: WheelEvent) => {
      // Vertical scroll only — no zoom (avoids conflict with browser zoom)
      const s = stateRef.current
      const maxScroll = Math.max(0, s.contentHeight - (s.height - TIME_AXIS_HEIGHT))
      s.scrollY = Math.max(0, Math.min(maxScroll, s.scrollY + e.deltaY))
      requestRender()
    },
    [requestRender]
  )

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      if (e.button !== 0) return
      const s = stateRef.current
      const rect = containerRef.current?.getBoundingClientRect()
      if (!rect) return

      // Left click: drag-to-zoom selection
      s.isDragging = true
      s.isPanning = false
      s.dragStartX = e.clientX - rect.left
      s.dragCurrentX = e.clientX - rect.left
      e.preventDefault()
      // Focus container so WASD keys work
      containerRef.current?.focus()
    },
    []
  )

  const handleMouseMove = useCallback(
    (e: React.MouseEvent) => {
      const s = stateRef.current
      const rect = containerRef.current?.getBoundingClientRect()
      if (!rect) return
      const x = e.clientX - rect.left
      const y = e.clientY - rect.top
      s.mouseX = x

      if (s.isDragging) {
        s.dragCurrentX = x
        requestRender()
        return
      }

      // Hover tooltip
      const tooltip = tooltipRef.current
      if (!tooltip) return

      const timePerPx = (s.viewMaxTime - s.viewMinTime) / s.width
      const dataX = s.viewMinTime + x * timePerPx
      const dataY = y + s.scrollY

      const hit = hitTest(index, dataX, dataY)
      if (hit) {
        const nameCol = index.table.getChild('name')!
        const beginCol = index.table.getChild('begin')!
        const endCol = index.table.getChild('end')!
        const idCol = index.table.getChild('id')!
        const parentCol = index.table.getChild('parent')!
        const depthCol = index.table.getChild('depth')!
        const beginField = index.table.schema.fields.find((f) => f.name === 'begin')
        const endField = index.table.schema.fields.find((f) => f.name === 'end')
        const targetCol = index.table.getChild('target')
        const filenameCol = index.table.getChild('filename')
        const lineCol = index.table.getChild('line')

        const name = String(nameCol.get(hit.rowIndex) ?? '')
        const begin = timestampToMs(beginCol.get(hit.rowIndex), beginField?.type)
        const end = timestampToMs(endCol.get(hit.rowIndex), endField?.type)
        const duration = end - begin
        const spanId = idCol.get(hit.rowIndex)
        const parentId = parentCol.get(hit.rowIndex)
        const depth = depthCol.get(hit.rowIndex)

        // Resolve parent name
        let parentName = ''
        if (parentId != null) {
          for (let r = 0; r < index.table.numRows; r++) {
            if (idCol.get(r) === parentId) {
              parentName = String(nameCol.get(r) ?? '')
              break
            }
          }
        }

        let info = `<b>${escapeHtml(name)}</b><br>Duration: ${formatDuration(duration)}`
        info += `<br>id: ${spanId}, depth: ${depth}`
        info += `<br>parent: ${parentId}${parentName ? ` (${escapeHtml(parentName)})` : ''}`
        if (targetCol) {
          const target = targetCol.get(hit.rowIndex)
          if (target) info += `<br>Target: ${escapeHtml(String(target))}`
        }
        if (filenameCol) {
          const filename = filenameCol.get(hit.rowIndex)
          const line = lineCol?.get(hit.rowIndex)
          if (filename) info += `<br>${escapeHtml(String(filename))}${line != null ? `:${line}` : ''}`
        }

        tooltip.innerHTML = info
        tooltip.style.display = 'block'
        // Position tooltip near cursor but keep in bounds
        const tooltipX = Math.min(x + 12, s.width - 200)
        const tooltipY = Math.min(y + 12, s.height - 80)
        tooltip.style.left = `${tooltipX}px`
        tooltip.style.top = `${tooltipY}px`
      } else {
        tooltip.style.display = 'none'
      }
    },
    [index, requestRender]
  )

  const handleMouseUp = useCallback(
    (e: React.MouseEvent) => {
      const s = stateRef.current
      if (!s.isDragging) return

      const minX = Math.min(s.dragStartX, s.dragCurrentX)
      const maxX = Math.max(s.dragStartX, s.dragCurrentX)

      if (maxX - minX > 5) {
        const timePerPx = (s.viewMaxTime - s.viewMinTime) / s.width
        const fromTime = s.viewMinTime + minX * timePerPx
        const toTime = s.viewMinTime + maxX * timePerPx

        if (e.altKey && onTimeRangeSelect) {
          // Alt+drag: propagate to notebook time range
          onTimeRangeSelect(new Date(fromTime), new Date(toTime))
        } else {
          // Regular drag: zoom into selection
          s.viewMinTime = fromTime
          s.viewMaxTime = toTime
        }
      }

      s.isDragging = false
      requestRender()
    },
    [onTimeRangeSelect, requestRender]
  )

  const handleMouseLeave = useCallback(() => {
    const s = stateRef.current
    if (s.isDragging) {
      s.isDragging = false
      requestRender()
    }
    const tooltip = tooltipRef.current
    if (tooltip) tooltip.style.display = 'none'
  }, [requestRender])

  const handleDoubleClick = useCallback(() => {
    // Reset zoom to full range
    const s = stateRef.current
    s.viewMinTime = index.timeRange.min
    s.viewMaxTime = index.timeRange.max
    requestRender()
  }, [index, requestRender])

  // WASD key listeners + wheel scroll
  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    const onKeyDown = (e: KeyboardEvent) => {
      const key = e.key.toLowerCase()
      if ('wasd'.includes(key)) {
        e.preventDefault()
        keysRef.current.add(key)
        if (!keyAnimRef.current) keyAnimRef.current = requestAnimationFrame(keyTick)
      }
    }
    const onKeyUp = (e: KeyboardEvent) => {
      keysRef.current.delete(e.key.toLowerCase())
    }

    container.addEventListener('keydown', onKeyDown)
    container.addEventListener('keyup', onKeyUp)
    container.addEventListener('wheel', handleWheel, { passive: true })
    const keys = keysRef.current
    return () => {
      container.removeEventListener('keydown', onKeyDown)
      container.removeEventListener('keyup', onKeyUp)
      container.removeEventListener('wheel', handleWheel)
      cancelAnimationFrame(keyAnimRef.current)
      keys.clear()
    }
  }, [handleWheel, keyTick])

  return (
    <div
      ref={containerRef}
      tabIndex={0}
      className="relative w-full h-full select-none outline-none overflow-hidden"
      style={{ cursor: 'crosshair' }}
      onMouseDown={handleMouseDown}
      onMouseMove={handleMouseMove}
      onMouseUp={handleMouseUp}
      onMouseLeave={handleMouseLeave}
      onDoubleClick={handleDoubleClick}
    >
      <canvas
        ref={webglCanvasRef}
        className="absolute top-0 left-0"
      />
      <canvas
        ref={textCanvasRef}
        className="absolute top-0 left-0 pointer-events-none"
      />
      <div
        ref={tooltipRef}
        className="absolute z-10 px-2 py-1 text-xs rounded shadow-lg pointer-events-none"
        style={{
          display: 'none',
          backgroundColor: 'rgba(15, 15, 30, 0.95)',
          color: '#e5e7eb',
          border: '1px solid rgba(75, 85, 99, 0.5)',
          whiteSpace: 'nowrap',
        }}
      />
    </div>
  )
}

function escapeHtml(str: string): string {
  return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

// =============================================================================
// Renderer Component
// =============================================================================

export function FlameGraphCell({
  data,
  status,
  onTimeRangeSelect,
}: CellRendererProps) {
  const table = data[0]

  const index = useMemo(() => {
    if (!table || table.numRows === 0) return null
    return buildFlameIndex(table)
  }, [table])

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center h-[200px]">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
      </div>
    )
  }

  if (index?.error) {
    return (
      <div className="flex items-center justify-center h-[200px] text-red-400 text-sm px-4 text-center">
        {index.error}
      </div>
    )
  }

  if (!index || !table || table.numRows === 0) {
    return (
      <div className="flex items-center justify-center h-[200px] text-theme-text-muted text-sm">
        No data available
      </div>
    )
  }

  return (
    <div className="flex-1 min-h-0 h-full">
      <FlameGraphView index={index} onTimeRangeSelect={onTimeRangeSelect} />
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function FlameGraphCellEditor({ config, onChange, variables, timeRange, onRun, cellResults, cellSelections }: CellEditorProps) {
  const fgConfig = config as QueryCellConfig

  const validationErrors = useMemo(() => {
    return validateMacros(fgConfig.sql, variables, cellResults, cellSelections).errors
  }, [fgConfig.sql, variables, cellResults, cellSelections])

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={fgConfig.sql}
          onChange={(sql) => onChange({ ...fgConfig, sql })}
          language="sql"
          placeholder="SELECT name, begin, end, depth, lane FROM ..."
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
      <AvailableVariablesPanel variables={variables} timeRange={timeRange} cellResults={cellResults} cellSelections={cellSelections} />
      <DocumentationLink url={QUERY_GUIDE_URL} label="Query Guide" />
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

// eslint-disable-next-line react-refresh/only-export-components
export const flamegraphMetadata: CellTypeMetadata = {
  renderer: FlameGraphCell,
  EditorComponent: FlameGraphCellEditor,

  label: 'Flame Graph',
  icon: <Flame />,
  description: 'Perfetto-style flame graph visualization of CPU traces',
  showTypeBadge: true,
  defaultHeight: 400,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'flamegraph' as const,
    sql: DEFAULT_SQL.flamegraph,
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
