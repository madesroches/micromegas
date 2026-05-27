import { useRef, useEffect, useCallback, useMemo, useState } from 'react'
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
import { parseRelativeTime } from '@/lib/time-range'
import { Flame, ChevronDown, ChevronRight } from 'lucide-react'
import {
  axisValue,
  buildFlameIndex,
  formatBits,
  formatDuration,
  hitTest,
  totalHeight,
  TIME_AXIS_HEIGHT,
  type FlameIndex,
} from './flame-model'
import { createFlameScene, type FlameScene } from './FlameGraphScene'

// =============================================================================
// FlameGraph Renderer (React shell over FlameGraphScene)
// =============================================================================

interface FlameGraphViewProps {
  index: FlameIndex
  onTimeRangeSelect?: (from: Date, to: Date) => void
  initialTimeRange?: { min: number; max: number }
}

function FlameGraphView({ index, onTimeRangeSelect, initialTimeRange }: FlameGraphViewProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const webglCanvasRef = useRef<HTMLCanvasElement>(null)
  const textCanvasRef = useRef<HTMLCanvasElement>(null)
  const tooltipRef = useRef<HTMLDivElement>(null)

  // The scene owns the WebGL resources; the shell owns the view state below.
  const sceneRef = useRef<FlameScene | null>(null)

  // View + interaction state stored in refs to avoid re-renders
  const stateRef = useRef({
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
  })

  const animFrameRef = useRef(0)

  // -----------------------------------------------------------------------
  // Frame request — snapshots the view state and hands it to the scene.
  // -----------------------------------------------------------------------
  const render = useCallback(() => {
    const scene = sceneRef.current
    if (!scene) return
    const s = stateRef.current
    scene.render(index, {
      viewMinTime: s.viewMinTime,
      viewMaxTime: s.viewMaxTime,
      scrollY: s.scrollY,
      isDragging: s.isDragging,
      isPanning: s.isPanning,
      dragStartX: s.dragStartX,
      dragCurrentX: s.dragCurrentX,
      width: s.width,
      height: s.height,
    })
  }, [index])

  const requestRender = useCallback(() => {
    cancelAnimationFrame(animFrameRef.current)
    animFrameRef.current = requestAnimationFrame(render)
  }, [render])

  // -----------------------------------------------------------------------
  // Scene setup / teardown
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

    const scene = createFlameScene(webglCanvas, textCanvas, Math.max(index.table.numRows, 1024))
    sceneRef.current = scene
    scene.resize(w, h, dpr)
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
      scene.resize(newW, newH, newDpr)
      requestRender()
    })
    resizeObserver.observe(container)

    return () => {
      resizeObserver.disconnect()
      cancelAnimationFrame(animFrameRef.current)
      scene.dispose()
      sceneRef.current = null
    }
  }, [index, requestRender])

  // -----------------------------------------------------------------------
  // Apply initial time range (separate from setup to avoid re-creating the scene)
  // -----------------------------------------------------------------------
  useEffect(() => {
    if (!initialTimeRange) return
    const s = stateRef.current
    s.viewMinTime = initialTimeRange.min
    s.viewMaxTime = initialTimeRange.max
    requestRender()
  }, [initialTimeRange, requestRender])

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
        const targetCol = index.table.getChild('target')
        const filenameCol = index.table.getChild('filename')
        const lineCol = index.table.getChild('line')
        const kindCol = index.table.getChild('kind')
        const bitSizeCol = index.table.getChild('bit_size')
        const connectionNameCol = index.table.getChild('connection_name')
        const isOutgoingCol = index.table.getChild('is_outgoing')

        const name = String(nameCol.get(hit.rowIndex) ?? '')
        const begin = axisValue(beginCol.get(hit.rowIndex), beginCol.type, index.xAxisMode)
        const end = axisValue(endCol.get(hit.rowIndex), endCol.type, index.xAxisMode)
        const spanId = idCol.get(hit.rowIndex)
        const parentId = parentCol.get(hit.rowIndex)
        const depth = depthCol.get(hit.rowIndex)

        // Resolve parent name via pre-built map (O(1) instead of O(n) scan)
        const parentName = (parentId != null && index.idToName.get(parentId)) || ''

        let info = `<b>${escapeHtml(name)}</b>`
        if (index.xAxisMode === 'bits') {
          info += `<br>Size: ${formatBits(end - begin)}`
          if (bitSizeCol) {
            const bs = bitSizeCol.get(hit.rowIndex)
            if (bs != null) info += ` (bit_size: ${formatBits(Number(bs))})`
          }
        } else {
          info += `<br>Duration: ${formatDuration(end - begin)}`
        }
        if (kindCol) {
          const kind = kindCol.get(hit.rowIndex)
          if (kind) info += `<br>Kind: ${escapeHtml(String(kind))}`
        }
        if (connectionNameCol) {
          const conn = connectionNameCol.get(hit.rowIndex)
          if (conn) info += `<br>Connection: ${escapeHtml(String(conn))}`
        }
        if (isOutgoingCol) {
          const out = isOutgoingCol.get(hit.rowIndex)
          if (out != null) info += `<br>Direction: ${out ? 'outgoing' : 'incoming'}`
        }
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

        // Alt+drag broadcasts the selection as a time range to downstream cells.
        // In bits mode the X-axis is bit counts, so feeding them through `new Date(...)`
        // would propagate meaningless timestamps. Only zoom locally in that case.
        if (e.altKey && onTimeRangeSelect && index.xAxisMode === 'time') {
          onTimeRangeSelect(new Date(fromTime), new Date(toTime))
        } else {
          // Regular drag (or bits mode): zoom into selection locally.
          s.viewMinTime = fromTime
          s.viewMaxTime = toTime
        }
      }

      s.isDragging = false
      requestRender()
    },
    [index, onTimeRangeSelect, requestRender]
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

  // WASD key listeners + wheel scroll.
  //
  // Identify the held key by `e.code` (`"KeyW"`/`"KeyA"`/`"KeyS"`/`"KeyD"`)
  // rather than `e.key`. On some Chrome/OS combinations `e.key` comes back as
  // `"Unidentified"` on keyup even though `e.code` is correct, which would
  // leave the key stuck in the set and `keyTick` running forever.
  //
  // `keyup` listens on `window` and we also clear on container blur / window
  // blur / `visibilitychange` (hidden) as a safety net for cases where the
  // OS-level release is delivered while focus has moved away.
  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    const keys = keysRef.current

    const clearAllKeys = () => {
      keys.clear()
      if (keyAnimRef.current) {
        cancelAnimationFrame(keyAnimRef.current)
        keyAnimRef.current = 0
      }
    }

    const codeToKey = (code: string): string | null => {
      switch (code) {
        case 'KeyW': return 'w'
        case 'KeyA': return 'a'
        case 'KeyS': return 's'
        case 'KeyD': return 'd'
        default: return null
      }
    }

    const onKeyDown = (e: KeyboardEvent) => {
      const key = codeToKey(e.code)
      if (key) {
        e.preventDefault()
        keys.add(key)
        if (!keyAnimRef.current) keyAnimRef.current = requestAnimationFrame(keyTick)
      }
    }
    const onKeyUp = (e: KeyboardEvent) => {
      const key = codeToKey(e.code)
      if (key) keys.delete(key)
    }
    const onVisibilityChange = () => {
      if (document.hidden) clearAllKeys()
    }

    container.addEventListener('keydown', onKeyDown)
    container.addEventListener('blur', clearAllKeys)
    container.addEventListener('wheel', handleWheel, { passive: true })
    window.addEventListener('keyup', onKeyUp)
    window.addEventListener('blur', clearAllKeys)
    document.addEventListener('visibilitychange', onVisibilityChange)

    return () => {
      container.removeEventListener('keydown', onKeyDown)
      container.removeEventListener('blur', clearAllKeys)
      container.removeEventListener('wheel', handleWheel)
      window.removeEventListener('keyup', onKeyUp)
      window.removeEventListener('blur', clearAllKeys)
      document.removeEventListener('visibilitychange', onVisibilityChange)
      clearAllKeys()
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
// Initial time range resolution
// =============================================================================

interface ResolvedInitialTimeRange {
  range?: { min: number; max: number }
  error?: string
}

function resolveInitialTimeRange(
  options: Record<string, unknown> | undefined,
  context: CellExecutionContext,
): ResolvedInitialTimeRange {
  const rawFrom = options?.initialFrom
  const rawTo = options?.initialTo
  const fromStr = typeof rawFrom === 'string' && rawFrom.trim() !== '' ? rawFrom.trim() : null
  const toStr = typeof rawTo === 'string' && rawTo.trim() !== '' ? rawTo.trim() : null

  if (!fromStr && !toStr) return {}

  const errors: string[] = []
  let min: number | undefined
  let max: number | undefined

  if (fromStr) {
    try {
      const resolved = substituteMacros(fromStr, context.variables, context.timeRange, context.cellResults, context.cellSelections)
      min = parseRelativeTime(resolved).getTime()
    } catch (e) {
      errors.push(`Invalid initial from: ${e instanceof Error ? e.message : String(e)}`)
    }
  }

  if (toStr) {
    try {
      const resolved = substituteMacros(toStr, context.variables, context.timeRange, context.cellResults, context.cellSelections)
      max = parseRelativeTime(resolved).getTime()
    } catch (e) {
      errors.push(`Invalid initial to: ${e instanceof Error ? e.message : String(e)}`)
    }
  }

  if (errors.length > 0) {
    return { error: errors.join('; ') }
  }

  // At least one bound was resolved — return partial range (caller fills missing bound from data)
  if (min != null || max != null) {
    return { range: { min: min ?? -Infinity, max: max ?? Infinity } }
  }

  return {}
}

// =============================================================================
// Renderer Component
// =============================================================================

export function FlameGraphCell({
  data,
  status,
  options,
  onTimeRangeSelect,
}: CellRendererProps) {
  const table = data[0]

  const index = useMemo(() => {
    if (!table || table.numRows === 0) return null
    return buildFlameIndex(table)
  }, [table])

  // Read pre-resolved initial time range from execute (stored in options by getRendererProps).
  // In bits mode the initialFrom/initialTo knobs are silently ignored — those parse as time,
  // while the X-axis is a bit count. Fit-to-data is the right default for bits.
  const isBitsMode = index?.xAxisMode === 'bits'
  const resolvedMin = !isBitsMode && typeof options?.resolvedInitialMin === 'number' ? options.resolvedInitialMin : undefined
  const resolvedMax = !isBitsMode && typeof options?.resolvedInitialMax === 'number' ? options.resolvedInitialMax : undefined
  const initialTimeRangeError = !isBitsMode && typeof options?.initialTimeRangeError === 'string' ? options.initialTimeRangeError : undefined

  // Fill missing bounds from data range
  const filledMin = resolvedMin === -Infinity && index ? index.timeRange.min : resolvedMin
  const filledMax = resolvedMax === Infinity && index ? index.timeRange.max : resolvedMax

  const initialTimeRange = useMemo(
    () => (filledMin != null && filledMax != null ? { min: filledMin, max: filledMax } : undefined),
    [filledMin, filledMax],
  )

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
    <div className="flex-1 min-h-0 h-full flex flex-col">
      {initialTimeRangeError && (
        <div className="px-3 py-2 bg-red-500/10 border border-red-500/30 rounded text-red-400 text-xs">
          {initialTimeRangeError}
        </div>
      )}
      <div className="flex-1 min-h-0">
        <FlameGraphView index={index} onTimeRangeSelect={onTimeRangeSelect} initialTimeRange={initialTimeRange} />
      </div>
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function FlameGraphCellEditor({ config, onChange, variables, timeRange, onRun, cellResults, cellSelections }: CellEditorProps) {
  const fgConfig = config as QueryCellConfig
  const [viewOptionsOpen, setViewOptionsOpen] = useState(
    Boolean((fgConfig.options?.initialFrom as string)?.trim() || (fgConfig.options?.initialTo as string)?.trim()),
  )

  const validationErrors = useMemo(() => {
    return validateMacros(fgConfig.sql, variables, cellResults, cellSelections).errors
  }, [fgConfig.sql, variables, cellResults, cellSelections])

  const handleOptionChange = useCallback(
    (key: string, value: string) => {
      onChange({ ...fgConfig, options: { ...fgConfig.options, [key]: value } })
    },
    [fgConfig, onChange],
  )

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
          placeholder="SELECT name, begin, end, depth, lane FROM ... (begin/end are timestamps for CPU traces, or bit offsets for net_spans)"
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
      <div>
        <button
          type="button"
          className="flex items-center gap-1 text-xs font-medium text-theme-text-secondary uppercase hover:text-theme-text-primary"
          onClick={() => setViewOptionsOpen((o) => !o)}
        >
          {viewOptionsOpen ? <ChevronDown className="w-3.5 h-3.5" /> : <ChevronRight className="w-3.5 h-3.5" />}
          View Options
        </button>
        {viewOptionsOpen && (
          <div className="mt-2 space-y-2 pl-1">
            <div className="flex items-center gap-2">
              <label className="text-xs text-theme-text-secondary w-20 shrink-0">Initial From</label>
              <input
                type="text"
                className="flex-1 bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary placeholder:text-theme-text-muted focus:outline-none focus:border-accent-link"
                placeholder="$from, now-1h, or variable"
                value={(fgConfig.options?.initialFrom as string) ?? ''}
                onChange={(e) => handleOptionChange('initialFrom', e.target.value)}
              />
            </div>
            <div className="flex items-center gap-2">
              <label className="text-xs text-theme-text-secondary w-20 shrink-0">Initial To</label>
              <input
                type="text"
                className="flex-1 bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary placeholder:text-theme-text-muted focus:outline-none focus:border-accent-link"
                placeholder="$to, now, or variable"
                value={(fgConfig.options?.initialTo as string) ?? ''}
                onChange={(e) => handleOptionChange('initialTo', e.target.value)}
              />
            </div>
          </div>
        )}
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
export const flamegraphMetadata: CellTypeMetadata = {
  renderer: FlameGraphCell,
  EditorComponent: FlameGraphCellEditor,

  label: 'Flame Graph',
  icon: <Flame />,
  description: 'Stack visualization for CPU or network traces',
  showTypeBadge: true,
  defaultHeight: 400,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'flamegraph' as const,
    sql: DEFAULT_SQL.flamegraph,
    options: {},
  }),

  execute: async (config: CellConfig, context: CellExecutionContext) => {
    const fgConfig = config as QueryCellConfig
    const sql = substituteMacros(fgConfig.sql, context.variables, context.timeRange, context.cellResults, context.cellSelections)
    const data = await context.runQuery(sql)
    const initialRange = resolveInitialTimeRange(fgConfig.options, context)
    const meta: Record<string, unknown> = {}
    if (initialRange.range) {
      meta.resolvedInitialMin = initialRange.range.min
      meta.resolvedInitialMax = initialRange.range.max
    }
    if (initialRange.error) {
      meta.initialTimeRangeError = initialRange.error
    }
    return { data: [data], meta }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
    options: { ...(config as QueryCellConfig).options, ...state.meta },
  }),
}
