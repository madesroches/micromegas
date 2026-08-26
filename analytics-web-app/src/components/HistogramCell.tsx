import { memo, useRef, useState } from 'react'
import type { StructRowProxy } from 'apache-arrow'
import {
  toHistogramValue,
  estimateHistogramQuantile,
  bucketRange,
  type HistogramValue,
} from '@/lib/histogram-utils'
import { resolveHistogramBarColor } from '@/lib/histogram-colors'

// =============================================================================
// Layout constants (mirrors tasks/histogram_column_cell_mockups/option-b-tick-median.html)
// =============================================================================

const CELL_WIDTH = 168
const CELL_HEIGHT = 28
const TRACK_WIDTH = 120
const TRACK_GAP = 6
const LABEL_WIDTH = 42

interface HistogramCellProps {
  /** The raw `Histogram` struct cell, or `null` for a null value / no data. */
  value: StructRowProxy | null
  /** `undefined` -> default flat `var(--chart-line)`; a colormap name or a
   *  literal CSS color otherwise (`ColumnOverride.histogramColor`). */
  color?: string
}

interface HoverState {
  x: number
  y: number
  bucket: number
}

/**
 * Custom equality for `React.memo`: the struct value `HistogramCell` receives
 * is a fresh `StructRowProxy` on every parent render (Arrow's `getStruct`/
 * `getList` getters allocate a new proxy/`Vector` on every access), so
 * comparing by reference never hits. Short-circuits before normalizing
 * anything when either side is null (`toHistogramValue` has no null branch);
 * otherwise both sides are normalized and compared by content.
 */
function arePropsEqual(prev: HistogramCellProps, next: HistogramCellProps): boolean {
  if (prev.color !== next.color) return false

  if (prev.value == null || next.value == null) {
    return prev.value == null && next.value == null
  }

  const a = toHistogramValue(prev.value)
  const b = toHistogramValue(next.value)
  return (
    a.start === b.start &&
    a.end === b.end &&
    a.count === b.count &&
    a.bins.length === b.bins.length &&
    a.bins.every((v, i) => v === b.bins[i])
  )
}

function HistogramCellImpl({ value, color }: HistogramCellProps) {
  const svgRef = useRef<SVGSVGElement>(null)
  const [hover, setHover] = useState<HoverState | null>(null)

  // Null value: matches formatCell's existing null convention. Also covers
  // the unconfigured-accumulator path (Arrow JS delivers `null`, not a
  // non-null struct with empty bins).
  if (value == null) {
    return <span className="text-theme-text-primary">-</span>
  }

  const h = toHistogramValue(value)

  // Degenerate: count === 0 (every sampled value was null) or every bucket
  // is 0 (equivalent, since sampling clamps every non-null value into
  // range, so sum(bins) === count). Rendering these as '-' avoids
  // 0/0 = NaN bar heights and tooltip percentages. The bins.length === 0
  // guard is cheap defense-in-depth — not a shape the Rust side produces.
  if (h.count === 0 || h.bins.length === 0) {
    return <span className="text-theme-text-primary">-</span>
  }

  const max = Math.max(...h.bins)
  const median = estimateHistogramQuantile(h, 0.5)
  const degeneratePoint = Math.abs(h.end - h.start) < Number.EPSILON
  const tickX = degeneratePoint ? 0 : ((median - h.start) / (h.end - h.start)) * TRACK_WIDTH

  const handlePointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const rect = svgRef.current?.getBoundingClientRect()
    if (!rect || rect.width === 0) return
    const x = e.clientX - rect.left
    const idx = Math.min(h.bins.length - 1, Math.max(0, Math.floor(x / (rect.width / h.bins.length))))
    setHover((prev) => (prev && prev.bucket === idx ? prev : { x: e.clientX, y: e.clientY, bucket: idx }))
  }

  const handlePointerLeave = () => setHover(null)

  const barWidth = TRACK_WIDTH / h.bins.length

  return (
    <div
      className="inline-flex items-center"
      style={{ width: CELL_WIDTH, height: CELL_HEIGHT, gap: TRACK_GAP }}
    >
      <svg
        ref={svgRef}
        data-testid="histogram-track"
        width={TRACK_WIDTH}
        height={CELL_HEIGHT}
        viewBox={`0 0 ${TRACK_WIDTH} ${CELL_HEIGHT}`}
        preserveAspectRatio="none"
        onPointerMove={handlePointerMove}
        onPointerLeave={handlePointerLeave}
        style={{ flex: `0 0 ${TRACK_WIDTH}px`, display: 'block' }}
      >
        {h.bins.map((bucketCount, i) => {
          const t = bucketCount / max
          const height = Math.max(2, t * CELL_HEIGHT)
          return (
            <rect
              key={i}
              x={i * barWidth}
              y={CELL_HEIGHT - height}
              width={barWidth}
              height={height}
              fill={resolveHistogramBarColor(color, t)}
            />
          )
        })}
        <line
          x1={tickX}
          x2={tickX}
          y1={0}
          y2={CELL_HEIGHT}
          stroke="var(--brand-gold)"
          strokeWidth={1.5}
        />
      </svg>
      <span
        data-testid="histogram-median"
        className="text-brand-gold font-mono whitespace-nowrap overflow-hidden text-ellipsis"
        style={{ flex: `0 0 ${LABEL_WIDTH}px`, textAlign: 'right', fontSize: '9.5px', lineHeight: 1 }}
      >
        {median.toFixed(1)}
      </span>
      {hover && (
        <HistogramTooltip x={hover.x} y={hover.y} h={h} bucket={hover.bucket} />
      )}
    </div>
  )
}

interface HistogramTooltipProps {
  x: number
  y: number
  h: HistogramValue
  bucket: number
}

function HistogramTooltip({ x, y, h, bucket }: HistogramTooltipProps) {
  const [rangeStart, rangeEnd] = bucketRange(h, bucket)
  const count = h.bins[bucket]
  const pct = ((count / h.count) * 100).toFixed(1)
  return (
    <div
      data-testid="histogram-tooltip"
      className="fixed bg-app-bg border border-theme-border rounded-md px-2.5 py-2 text-xs font-mono pointer-events-none z-50 shadow-lg"
      style={{ left: x + 14, top: y - 10 }}
    >
      <div className="text-theme-text-muted text-[10px] mb-1">bucket</div>
      <div className="text-theme-text-primary font-semibold">
        {rangeStart.toFixed(1)}–{rangeEnd.toFixed(1)}
      </div>
      <div className="text-theme-text-secondary text-[10px] mt-0.5">
        count: {count} ({pct}%)
      </div>
    </div>
  )
}

/**
 * Renders a per-row histogram bar chart with a median tick and hover
 * tooltip, inside a `<td>` (same pattern as `OverrideCell` — a component,
 * not a formatted string). Wrapped in `React.memo` with a structural
 * comparator (see `arePropsEqual`) rather than `useMemo` keyed on the value
 * prop, since the value's identity is fresh on every parent render.
 */
export const HistogramCell = memo(HistogramCellImpl, arePropsEqual)
