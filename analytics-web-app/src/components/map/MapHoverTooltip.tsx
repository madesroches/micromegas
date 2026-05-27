import { useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import type { DataType, Table } from 'apache-arrow'
import { EventDetailContent } from './EventDetailContent'
import type { VariableValue } from '@/lib/screen-renderers/notebook-types'

interface MapHoverTooltipProps {
  x: number
  y: number
  row: Record<string, unknown>
  columnTypes: Map<string, DataType>
  template: string
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
}

// Cursor offset and viewport margin. Tuned defaults — the tooltip sits down-
// right of the cursor and flips/clamps near edges.
const CURSOR_OFFSET = 14
const VIEWPORT_MARGIN = 8

/**
 * Transient detail-template preview that follows the cursor while a marker is
 * hovered. Portals to `document.body` and uses `position: fixed` so it escapes
 * the cell's `overflow-hidden` clip and any transformed ancestor; never
 * intercepts pointer events (that would steal the move and flicker).
 */
export function MapHoverTooltip({
  x,
  y,
  row,
  columnTypes,
  template,
  variables,
  timeRange,
  cellResults,
  cellSelections,
}: MapHoverTooltipProps) {
  const ref = useRef<HTMLDivElement>(null)
  // Default to the down-right offset; the layout effect corrects it pre-paint
  // against the measured size so flips/clamps don't visibly jump.
  const [pos, setPos] = useState({ left: x + CURSOR_OFFSET, top: y + CURSOR_OFFSET })

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    const vw = window.innerWidth
    const vh = window.innerHeight

    // Right of cursor by default; flip to the left if it would overflow.
    let left = x + CURSOR_OFFSET
    if (left + rect.width > vw - VIEWPORT_MARGIN) {
      left = x - CURSOR_OFFSET - rect.width
    }
    // Below cursor by default; flip above if it would overflow.
    let top = y + CURSOR_OFFSET
    if (top + rect.height > vh - VIEWPORT_MARGIN) {
      top = y - CURSOR_OFFSET - rect.height
    }
    // Final clamp so a tooltip larger than the gap still stays on-screen.
    left = Math.max(VIEWPORT_MARGIN, Math.min(left, vw - rect.width - VIEWPORT_MARGIN))
    top = Math.max(VIEWPORT_MARGIN, Math.min(top, vh - rect.height - VIEWPORT_MARGIN))
    setPos({ left, top })
  }, [x, y])

  return createPortal(
    <div
      ref={ref}
      className="fixed pointer-events-none z-50 w-fit max-w-[50%] max-h-[60%] overflow-hidden bg-app-panel border border-theme-border rounded-lg shadow-lg"
      style={{ left: pos.left, top: pos.top }}
    >
      <EventDetailContent
        row={row}
        columnTypes={columnTypes}
        template={template}
        variables={variables}
        timeRange={timeRange}
        cellResults={cellResults}
        cellSelections={cellSelections}
      />
    </div>,
    document.body,
  )
}
