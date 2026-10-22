export interface ChartPointHit {
  seriesIndex: number // 0-based index into XYChart's normalized series (== query index)
  rowIndex: number // source Arrow row
}

export interface BarRect {
  seriesIndex: number
  dataIdx: number
  left: number
  top: number
  width: number
  height: number
}

export const CLICK_THRESHOLD = 4
export const POINT_HIT_RADIUS = 8
export const DIM_FILL = 0.25
export const DIM_STROKE = 0.5
export const DIM_LINE = 0.3
export const DIM_POINT = 0.3

/** Topmost rect containing (x, y), in canvas pixels. */
export function hitTestBars(rects: BarRect[], x: number, y: number): BarRect | null {
  for (let i = rects.length - 1; i >= 0; i--) {
    const r = rects[i]
    if (x >= r.left && x <= r.left + r.width && y >= r.top && y <= r.top + r.height) {
      return r
    }
  }
  return null
}

/** Nearest candidate within `radius` CSS px, or null. The first of equidistant candidates wins. */
export function findNearestPoint<T extends { px: number; py: number }>(
  candidates: T[],
  x: number,
  y: number,
  radius: number,
): T | null {
  let best: T | null = null
  let bestDist = Infinity
  for (const c of candidates) {
    const d = Math.hypot(c.px - x, c.py - y)
    if (d <= radius && d < bestDist) {
      best = c
      bestDist = d
    }
  }
  return best
}

/** A mouseup this close to its mousedown counts as a click, not a drag-to-zoom. */
export function isClick(
  down: { x: number; y: number },
  up: { x: number; y: number },
  threshold: number = CLICK_THRESHOLD,
): boolean {
  return Math.hypot(up.x - down.x, up.y - down.y) < threshold
}

export function sameHit(a: ChartPointHit | null, b: ChartPointHit | null): boolean {
  if (a === null || b === null) return a === b
  return a.seriesIndex === b.seriesIndex && a.rowIndex === b.rowIndex
}

/** Clicking the selected mark again clears the selection. */
export function nextSelection(current: ChartPointHit | null, hit: ChartPointHit | null): ChartPointHit | null {
  if (hit === null) return null
  return sameHit(current, hit) ? null : hit
}

/**
 * Locate the selected mark in the current uPlot data arrays. A stale selection
 * (series out of range, or a row no longer present) resolves to null.
 */
export function resolveSelection(
  selected: ChartPointHit | null,
  rowIndexBySeries: (number | null)[][],
): { seriesIndex: number; dataIdx: number } | null {
  if (!selected) return null
  const rows = rowIndexBySeries[selected.seriesIndex]
  if (!rows) return null
  const dataIdx = rows.indexOf(selected.rowIndex)
  return dataIdx < 0 ? null : { seriesIndex: selected.seriesIndex, dataIdx }
}

/** Multiply the alpha of a CSS color (`#rrggbb`, `#rrggbbaa`, or `rgba(...)`) by `factor`. */
export function fadeCss(css: string, factor: number): string {
  if (factor === 1) return css
  const hex = css.trim().match(/^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})?$/i)
  if (hex) {
    const a = hex[4] !== undefined ? parseInt(hex[4], 16) / 255 : 1
    return `rgba(${parseInt(hex[1], 16)}, ${parseInt(hex[2], 16)}, ${parseInt(hex[3], 16)}, ${a * factor})`
  }
  const fn = css.trim().match(/^rgba?\(\s*([^,\s]+)[,\s]+([^,\s]+)[,\s]+([^,\s/)]+)(?:\s*[,/]\s*([^)\s]+))?\s*\)$/i)
  if (fn) {
    const a = fn[4] !== undefined ? (fn[4].endsWith('%') ? parseFloat(fn[4]) / 100 : parseFloat(fn[4])) : 1
    return `rgba(${fn[1]}, ${fn[2]}, ${fn[3]}, ${a * factor})`
  }
  return css
}

/** Per-bar colors for one series: the base colors, faded for every bar except the selected one. */
export function barColors(
  base: string[],
  selectedDataIdx: number | null,
  seriesIndex: number,
  selectedSeriesIndex: number | null,
  factor: number = DIM_FILL,
): string[] {
  if (selectedDataIdx === null || selectedSeriesIndex === null) return base
  return base.map((c, i) =>
    seriesIndex === selectedSeriesIndex && i === selectedDataIdx ? c : fadeCss(c, factor),
  )
}

/** `fadeCss` applied to each gradient stop's color. */
export function fadeStops(
  stops: { x: number; color: string }[],
  factor: number,
): { x: number; color: string }[] {
  return stops.map((s) => ({ x: s.x, color: fadeCss(s.color, factor) }))
}
