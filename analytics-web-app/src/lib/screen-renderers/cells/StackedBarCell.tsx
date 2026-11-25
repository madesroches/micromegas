import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
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
import { extractStackedBarData, type StackedBarData } from '@/lib/arrow-utils'
import { formatValueWithUnit } from '@/lib/format-value'
import { SERIES_COLORS } from '@/components/chart-constants'
import { contrastingTextColor } from '@/lib/color-utils'
import { estimateLabelWidth, ROTATE_DEG } from '@/components/xychart-axis'
import { DIM_FILL } from '@/components/xychart-selection'
import { RowSelectionEditor } from '@/components/RowSelectionEditor'
import { useLatestRef } from '@/hooks/useLatestRef'
import { ChartColumnStacked } from 'lucide-react'

// =============================================================================
// Series colors (pure)
// =============================================================================

export interface ResolvedStackedBarSeries {
  name: string
  color: string
}

/**
 * A SQL-supplied color wins; otherwise each series takes the next `SERIES_COLORS`
 * entry in series (first-appearance) order, wrapping after 12 — the same rule as
 * `groupPieSlices`, minus the "Other" folding (no series cap here).
 */
// eslint-disable-next-line react-refresh/only-export-components
export function resolveSeriesColors(series: { name: string; color?: string }[]): ResolvedStackedBarSeries[] {
  let paletteIdx = 0
  return series.map((s) => {
    if (s.color) return { name: s.name, color: s.color }
    const color = SERIES_COLORS[paletteIdx % SERIES_COLORS.length]
    paletteIdx++
    return { name: s.name, color }
  })
}

// =============================================================================
// Layout math (pure, unit-testable)
// =============================================================================

const BAR_MAX_WIDTH = 56
const BAR_MIN_WIDTH = 12
const BAR_WIDTH_FRACTION = 0.62
/** Narrowest band that still keeps the bar at `BAR_MIN_WIDTH` (band * fraction = min width). */
const MIN_BAND_WIDTH = BAR_MIN_WIDTH / BAR_WIDTH_FRACTION
const SEGMENT_GAP_PX = 2
const CAP_RADIUS_PX = 4
const MIN_LABEL_SEGMENT_HEIGHT = 18
const LABEL_TEXT_PADDING = 8
const TOP_MARGIN_PX = 10
/** Extra headroom above the tallest vertical bar so its total label isn't clipped. */
const TOTAL_LABEL_HEADROOM_PX = 14
const TOTAL_LABEL_GAP_PX = 4
const RIGHT_MARGIN_PX = 12
const X_LABEL_AREA_HEIGHT = 24
const X_LABEL_AREA_HEIGHT_ROTATED = 64
/** Gap below the plot's bottom edge before a rotated label's anchor point (its unrotated top). */
const ROTATED_LABEL_GUTTER_PX = 8
/**
 * Longest a rotated label may run (measured along the text, pre-rotation) before it's
 * truncated: past this, a -45deg label's vertical extent (length * sin(45deg)) would exceed
 * the space between its anchor and the label area's bottom edge.
 */
const MAX_ROTATED_LABEL_WIDTH_PX =
  (X_LABEL_AREA_HEIGHT_ROTATED - ROTATED_LABEL_GUTTER_PX) / Math.sin((Math.abs(ROTATE_DEG) * Math.PI) / 180)
/** Gutter chrome (tick marks, label padding) added on top of the widest tick label. */
const Y_AXIS_CHROME_PX = 28
const Y_TICK_DIVISIONS = 5

const ROW_HEIGHT = 28
const BAR_THICKNESS = 18
const AXIS_HEIGHT = 24
const LABEL_COLUMN_MAX = 180
const LABEL_COLUMN_PADDING = 16
const ROWS_VERTICAL_PADDING = 8
const MIN_TICK_DIVISIONS = 2
const TICK_LABEL_SPACING_PX = 24
const TOTAL_LABEL_ROOM_PADDING = 8
const HORIZONTAL_TOTAL_LABEL_GAP_PX = 6

export type BarOrientation = 'vertical' | 'horizontal'

/** Smallest of {1, 2, 2.5, 5} x 10^n that is >= `rawStep`. Mirrors XYChart's y-scale. */
// eslint-disable-next-line react-refresh/only-export-components
export function niceStep(rawStep: number): number {
  if (!(rawStep > 0)) return 1
  const exponent = Math.floor(Math.log10(rawStep))
  const p = Math.pow(10, exponent)
  const n = rawStep / p
  const nice = n <= 1 ? 1 : n <= 2 ? 2 : n <= 2.5 ? 2.5 : n <= 5 ? 5 : 10
  return nice * p
}

export interface StackedBarSegmentLayout {
  seriesIndex: number
  value: number
  x: number
  y: number
  width: number
  height: number
  /** Only the end-most non-zero segment of a bar is rounded; the baseline stays square. */
  rounded: boolean
  /** Whether the in-segment value label fits (vertical: 18px min height and bar width; horizontal: segment width). */
  labelFits: boolean
}

export interface StackedBarTotalLabel {
  x: number
  y: number
  text: string
}

export interface StackedBarBarLayout {
  category: string
  /** `category`, truncated with an ellipsis when it would otherwise overrun its label area; equal to `category` otherwise. */
  displayLabel: string
  /** Vertical only (0 when horizontal). */
  x: number
  /** Vertical only (0 when horizontal). */
  width: number
  total: number
  /** Vertical only (0 when horizontal). */
  labelX: number
  /** Horizontal only: top of the bar's row (0 when vertical). */
  rowY: number
  /** Horizontal only: category-label anchor, the row center (0 when vertical). */
  labelY: number
  segments: StackedBarSegmentLayout[]
  totalLabel: StackedBarTotalLabel | null
  /** The category's full clickable band in plot-svg coordinates. */
  hitBand: { x: number; y: number; width: number; height: number }
}

export interface StackedBarValueTick {
  value: number
  /** y for vertical, x for horizontal. */
  pos: number
  label: string
}

export interface StackedBarLayout {
  orientation: BarOrientation
  bars: StackedBarBarLayout[]
  valueTicks: StackedBarValueTick[]
  /** Content width the plot must render at; exceeds `width` when vertical bars would otherwise go below the floor, triggering horizontal scroll. */
  contentWidth: number
  contentHeight: number
  /** Vertical only (0 when horizontal). */
  yAxisWidth: number
  /** Vertical only (0 when horizontal). */
  plotHeight: number
  /** Vertical only (0 when horizontal). */
  topMargin: number
  /** Vertical only (0 when horizontal). */
  barWidth: number
  /** Vertical only (false when horizontal). */
  rotateLabels: boolean
  /** Horizontal only: the value-axis origin, where the category labels end (0 when vertical). */
  labelColumnWidth: number
  /** Horizontal only (0 when vertical). */
  barThickness: number
}

/** Truncates `label` with an ellipsis so its estimated width fits within `maxWidth`. */
// eslint-disable-next-line react-refresh/only-export-components
export function truncateLabel(label: string, maxWidth: number): string {
  if (estimateLabelWidth(label) <= maxWidth) return label
  let end = label.length
  while (end > 0 && estimateLabelWidth(label.slice(0, end) + '…') > maxWidth) {
    end--
  }
  return label.slice(0, end) + '…'
}

/**
 * Value axis shared by both orientations. Mirrors XYChart's y-scale (max x 1.05,
 * free ticks — no snapping/epsilon).
 */
// eslint-disable-next-line react-refresh/only-export-components
export function buildValueScale(
  maxTotal: number,
  unit: string,
  divisions: number,
): { top: number; step: number; ticks: { value: number; label: string }[] } {
  const top = maxTotal > 0 ? maxTotal * 1.05 : 1
  const step = niceStep(top / divisions)
  const tickCount = Math.floor(top / step + 1e-9) + 1
  const ticks: { value: number; label: string }[] = []
  for (let i = 0; i < tickCount; i++) {
    ticks.push({ value: step * i, label: formatValueWithUnit(step * i, unit) })
  }
  return { top, step, ticks }
}

interface StackArgs {
  values: number[]
  unit: string
  /** Maps a value to its pixel position along the stacking axis. */
  toPx: (v: number) => number
  /** Fixed cross-axis origin: x for vertical bars, y for horizontal bars. */
  cross: number
  /** Cross-axis size: bar width for vertical, bar thickness for horizontal. */
  thickness: number
  orientation: BarOrientation
}

function stackSegments({ values, unit, toPx, cross, thickness, orientation }: StackArgs): StackedBarSegmentLayout[] {
  const present = values.map((value, si) => ({ si, value })).filter((entry) => entry.value > 0)
  let acc = 0
  return present.map((entry, idx) => {
    const isEnd = idx === present.length - 1
    const p0 = toPx(acc)
    acc += entry.value
    const p1 = toPx(acc)
    const gap = isEnd ? 0 : SEGMENT_GAP_PX
    const textWidth = estimateLabelWidth(formatValueWithUnit(entry.value, unit)) + LABEL_TEXT_PADDING
    if (orientation === 'vertical') {
      const height = Math.max(p0 - p1 - gap, 0)
      return {
        seriesIndex: entry.si,
        value: entry.value,
        x: cross,
        y: p1 + gap,
        width: thickness,
        height,
        rounded: isEnd,
        labelFits: height >= MIN_LABEL_SEGMENT_HEIGHT && textWidth <= thickness,
      }
    }
    const width = Math.max(p1 - p0 - gap, 0)
    return {
      seriesIndex: entry.si,
      value: entry.value,
      x: p0,
      y: cross,
      width,
      height: thickness,
      rounded: isEnd,
      labelFits: textWidth <= width,
    }
  })
}

type BarInput = { categories: string[]; series: ResolvedStackedBarSeries[]; values: number[][] }

function barTotals({ categories, values }: BarInput): number[] {
  return categories.map((_, c) => values[c].reduce((a, b) => a + b, 0))
}

/**
 * Bar/segment rectangles, y ticks, and x-label placement for the vertical bar plot.
 * Mirrors the Pie cell's mark spec (surface-gap stroke, rounded cap only on the
 * outermost mark).
 */
// eslint-disable-next-line react-refresh/only-export-components
export function buildStackedBarLayout(
  resolved: BarInput,
  opts: { width: number; height: number; unit: string; valueLabels?: boolean },
): StackedBarLayout {
  const { categories, series, values } = resolved
  const { width, height, unit, valueLabels = false } = opts

  const totals = barTotals(resolved)
  const maxTotal = Math.max(0, ...totals)
  const scale = buildValueScale(maxTotal, unit, Y_TICK_DIVISIONS)
  const maxTickLabelWidth = Math.max(0, ...scale.ticks.map((t) => estimateLabelWidth(t.label)))
  const yAxisWidth = Math.ceil(maxTickLabelWidth) + Y_AXIS_CHROME_PX

  // Rotate when a label would overlap its neighbor at the *natural* (pre-floor) band width.
  const naturalPlotWidth = Math.max(0, width - yAxisWidth - RIGHT_MARGIN_PX)
  const naturalBand = categories.length > 0 ? naturalPlotWidth / categories.length : 0
  const maxCategoryLabelWidth = Math.max(0, ...categories.map(estimateLabelWidth))
  const rotateLabels = categories.length > 0 && maxCategoryLabelWidth > naturalBand

  const band = Math.max(naturalBand, MIN_BAND_WIDTH)
  const barWidth = Math.min(BAR_MAX_WIDTH, band * BAR_WIDTH_FRACTION)
  const contentWidth = Math.max(width, yAxisWidth + RIGHT_MARGIN_PX + band * categories.length)

  const topMargin = TOP_MARGIN_PX + (valueLabels ? TOTAL_LABEL_HEADROOM_PX : 0)
  const labelAreaHeight = rotateLabels ? X_LABEL_AREA_HEIGHT_ROTATED : X_LABEL_AREA_HEIGHT
  const plotHeight = Math.max(0, height - topMargin - labelAreaHeight)

  const yForValue = (v: number) => topMargin + plotHeight - (v / scale.top) * plotHeight

  const valueTicks: StackedBarValueTick[] = scale.ticks.map((t) => ({
    value: t.value,
    pos: yForValue(t.value),
    label: t.label,
  }))

  const bars: StackedBarBarLayout[] = categories.map((category, c) => {
    const x = yAxisWidth + band * c + (band - barWidth) / 2
    const total = totals[c]
    const segments = stackSegments({
      values: series.map((_, si) => values[c][si]),
      unit,
      toPx: yForValue,
      cross: x,
      thickness: barWidth,
      orientation: 'vertical',
    })

    const displayLabel = rotateLabels ? truncateLabel(category, MAX_ROTATED_LABEL_WIDTH_PX) : category
    const labelX = x + barWidth / 2

    let totalLabel: StackedBarTotalLabel | null = null
    if (valueLabels && total > 0) {
      const text = formatValueWithUnit(total, unit)
      // Skipped when it would collide with a neighbor's; the tooltip still shows the total.
      if (estimateLabelWidth(text) <= band) {
        totalLabel = { x: labelX, y: yForValue(total) - TOTAL_LABEL_GAP_PX, text }
      }
    }

    return {
      category,
      displayLabel,
      x,
      width: barWidth,
      total,
      labelX,
      rowY: 0,
      labelY: 0,
      segments,
      totalLabel,
      hitBand: { x: yAxisWidth + band * c, y: 0, width: band, height },
    }
  })

  return {
    orientation: 'vertical',
    bars,
    valueTicks,
    contentWidth,
    contentHeight: height,
    yAxisWidth,
    plotHeight,
    topMargin,
    barWidth,
    rotateLabels,
    labelColumnWidth: 0,
    barThickness: 0,
  }
}

/** Fixed-height rows, a label column on the left and the value axis running left to right. */
// eslint-disable-next-line react-refresh/only-export-components
export function buildHorizontalBarLayout(
  resolved: BarInput,
  opts: { width: number; unit: string; valueLabels?: boolean },
): StackedBarLayout {
  const { categories, series, values } = resolved
  const { width, unit, valueLabels = false } = opts

  const totals = barTotals(resolved)
  const maxTotal = Math.max(0, ...totals)

  const maxLabelWidth = Math.max(0, ...categories.map(estimateLabelWidth))
  const labelColumnWidth = Math.min(LABEL_COLUMN_MAX, Math.ceil(maxLabelWidth) + LABEL_COLUMN_PADDING)

  const totalTexts = totals.map((t) => formatValueWithUnit(t, unit))
  const totalLabelRoom = valueLabels
    ? Math.ceil(Math.max(0, ...totalTexts.map(estimateLabelWidth))) + TOTAL_LABEL_ROOM_PADDING
    : 0
  const plotWidth = Math.max(0, width - labelColumnWidth - RIGHT_MARGIN_PX - totalLabelRoom)

  // Tick labels sit side by side, so a narrow plot gets fewer of them.
  const widestTick = estimateLabelWidth(formatValueWithUnit(maxTotal > 0 ? maxTotal * 1.05 : 1, unit))
  const divisions = Math.min(
    Y_TICK_DIVISIONS,
    Math.max(MIN_TICK_DIVISIONS, Math.floor(plotWidth / (widestTick + TICK_LABEL_SPACING_PX))),
  )
  const scale = buildValueScale(maxTotal, unit, divisions)

  const xForValue = (v: number) => labelColumnWidth + (v / scale.top) * plotWidth

  const valueTicks: StackedBarValueTick[] = scale.ticks.map((t) => ({
    value: t.value,
    pos: xForValue(t.value),
    label: t.label,
  }))

  const barInset = (ROW_HEIGHT - BAR_THICKNESS) / 2
  const bars: StackedBarBarLayout[] = categories.map((category, c) => {
    const rowY = ROWS_VERTICAL_PADDING / 2 + c * ROW_HEIGHT
    const segments = stackSegments({
      values: series.map((_, si) => values[c][si]),
      unit,
      toPx: xForValue,
      cross: rowY + barInset,
      thickness: BAR_THICKNESS,
      orientation: 'horizontal',
    })
    const labelY = rowY + ROW_HEIGHT / 2
    const total = totals[c]
    const totalLabel: StackedBarTotalLabel | null =
      valueLabels && total > 0
        ? { x: xForValue(total) + HORIZONTAL_TOTAL_LABEL_GAP_PX, y: labelY, text: totalTexts[c] }
        : null
    return {
      category,
      displayLabel: truncateLabel(category, labelColumnWidth - LABEL_COLUMN_PADDING),
      x: 0,
      width: 0,
      total,
      labelX: 0,
      rowY,
      labelY,
      segments,
      totalLabel,
      hitBand: { x: 0, y: rowY, width, height: ROW_HEIGHT },
    }
  })

  return {
    orientation: 'horizontal',
    bars,
    valueTicks,
    contentWidth: width,
    contentHeight: categories.length * ROW_HEIGHT + ROWS_VERTICAL_PADDING,
    yAxisWidth: 0,
    plotHeight: 0,
    topMargin: 0,
    barWidth: 0,
    rotateLabels: false,
    labelColumnWidth,
    barThickness: BAR_THICKNESS,
  }
}

/** SVG path for a rect with two rounded corners at `end` and square corners at the baseline. */
function roundedEndRectPath(
  x: number,
  y: number,
  width: number,
  height: number,
  radius: number,
  end: 'top' | 'right',
): string {
  if (end === 'top') {
    const r = Math.min(radius, height, width / 2)
    return [
      `M ${x} ${y + height}`,
      `V ${y + r}`,
      `Q ${x} ${y} ${x + r} ${y}`,
      `H ${x + width - r}`,
      `Q ${x + width} ${y} ${x + width} ${y + r}`,
      `V ${y + height}`,
      'Z',
    ].join(' ')
  }
  const r = Math.min(radius, width, height / 2)
  return [
    `M ${x} ${y}`,
    `H ${x + width - r}`,
    `Q ${x + width} ${y} ${x + width} ${y + r}`,
    `V ${y + height - r}`,
    `Q ${x + width} ${y + height} ${x + width - r} ${y + height}`,
    `H ${x}`,
    'Z',
  ].join(' ')
}

// =============================================================================
// Selection (pure)
// =============================================================================

export interface BarHit {
  categoryIndex: number
  seriesIndex: number
}

function sameBarHit(a: BarHit | null, b: BarHit): boolean {
  return a !== null && a.categoryIndex === b.categoryIndex && a.seriesIndex === b.seriesIndex
}

/** Row published as `$cell.selected.*`, keyed by the query's SQL column names. */
// eslint-disable-next-line react-refresh/only-export-components
export function buildBarSelection(
  data: Pick<StackedBarData, 'categoryValues' | 'series' | 'values' | 'columns'>,
  hit: BarHit,
): Record<string, unknown> {
  const { columns } = data
  return {
    [columns.category]: data.categoryValues[hit.categoryIndex],
    ...(columns.series ? { [columns.series]: data.series[hit.seriesIndex].name } : {}),
    [columns.value]: data.values[hit.categoryIndex][hit.seriesIndex],
  }
}

/** Series index of the segment nearest to `pos` along the value axis; null when the bar has no segments. */
// eslint-disable-next-line react-refresh/only-export-components
export function nearestSegmentSeries(bar: StackedBarBarLayout, orientation: BarOrientation, pos: number): number | null {
  let best: number | null = null
  let bestDist = Infinity
  for (const seg of bar.segments) {
    const lo = orientation === 'vertical' ? seg.y : seg.x
    const hi = lo + (orientation === 'vertical' ? seg.height : seg.width)
    const dist = pos < lo ? lo - pos : pos > hi ? pos - hi : 0
    if (dist < bestDist) {
      best = seg.seriesIndex
      bestDist = dist
    }
  }
  return best
}

// =============================================================================
// Renderer Component
// =============================================================================

interface HoverState {
  category: string
  seriesName: string
  color: string
  value: number
  total: number
  x: number
  y: number
}

type ResolvedBarData = StackedBarData & { series: ResolvedStackedBarSeries[] }

interface StackedBarViewProps {
  resolvedData: ResolvedBarData
  resolvedUnit: string
  maxTotal: number
  orientation: BarOrientation
  valueLabels: boolean
  selected: BarHit | null
  /** Present only when selection is enabled. */
  onSegmentClick?: (hit: BarHit) => void
  onBackgroundClick?: () => void
}

/**
 * Owns the measured plot area (ResizeObserver) and hover state. Mounted only once
 * data is ready to render (see `StackedBarCell`'s success-path return), so the
 * ResizeObserver attaches to an already-present div — mirrors `FlameGraphView` in
 * FlameGraphCell.tsx.
 */
function StackedBarView({
  resolvedData,
  resolvedUnit,
  maxTotal,
  orientation,
  valueLabels,
  selected,
  onSegmentClick,
  onBackgroundClick,
}: StackedBarViewProps) {
  const plotRef = useRef<HTMLDivElement>(null)
  const [dimensions, setDimensions] = useState({ width: 0, height: 0 })
  const horizontal = orientation === 'horizontal'

  useEffect(() => {
    const el = plotRef.current
    if (!el) return
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0]
      if (!entry) return
      setDimensions({ width: entry.contentRect.width, height: entry.contentRect.height })
    })
    observer.observe(el)
    return () => observer.disconnect()
  }, [])

  const layout = useMemo(() => {
    if (resolvedData.categories.length === 0 || maxTotal === 0) return null
    if (dimensions.width === 0) return null
    if (horizontal) {
      return buildHorizontalBarLayout(resolvedData, { width: dimensions.width, unit: resolvedUnit, valueLabels })
    }
    if (dimensions.height === 0) return null
    return buildStackedBarLayout(resolvedData, {
      width: dimensions.width,
      height: dimensions.height,
      unit: resolvedUnit,
      valueLabels,
    })
  }, [resolvedData, maxTotal, dimensions, resolvedUnit, horizontal, valueLabels])

  const [hover, setHover] = useState<HoverState | null>(null)

  // Anchor point for a rotated label: the top of the label area (just below the
  // plot), not the bottom of the SVG — end-anchored text rotated -45deg hangs
  // down-left from there, staying within X_LABEL_AREA_HEIGHT_ROTATED instead of
  // being clipped past the bottom edge.
  const rotatedLabelY = layout ? layout.topMargin + layout.plotHeight + ROTATED_LABEL_GUTTER_PX : 0

  const handleBackgroundClick = onBackgroundClick
    ? (e: React.MouseEvent<SVGSVGElement>) => {
        if (!(e.target as Element).closest('[data-bar-hit]')) onBackgroundClick()
      }
    : undefined

  const handleBandClick = onSegmentClick
    ? (bi: number, e: React.MouseEvent<SVGElement>) => {
        const svg = e.currentTarget.ownerSVGElement
        const bar = layout?.bars[bi]
        if (!svg || !bar) return
        const rect = svg.getBoundingClientRect()
        const pos = horizontal ? e.clientX - rect.left : e.clientY - rect.top
        const seriesIndex = nearestSegmentSeries(bar, orientation, pos)
        if (seriesIndex !== null) onSegmentClick({ categoryIndex: bi, seriesIndex })
      }
    : undefined

  const hitRects =
    layout && handleBandClick
      ? layout.bars.map((bar, bi) =>
          bar.segments.length > 0 ? (
            <rect
              key={bi}
              x={bar.hitBand.x}
              y={bar.hitBand.y}
              width={bar.hitBand.width}
              height={bar.hitBand.height}
              fill="transparent"
              data-bar-hit
              className="cursor-pointer"
              onClick={(e) => handleBandClick(bi, e)}
            />
          ) : null,
        )
      : null

  const labelHitProps = (bi: number) =>
    handleBandClick && layout && layout.bars[bi].segments.length > 0
      ? {
          'data-bar-hit': true,
          className: 'cursor-pointer',
          onClick: (e: React.MouseEvent<SVGElement>) => handleBandClick(bi, e),
        }
      : {}

  const segmentColor = (c: number, s: number) => resolvedData.categoryColors?.[c] ?? resolvedData.series[s].color

  const segments = layout
    ? layout.bars.map((bar, bi) => (
        <g key={bi}>
          {bar.segments.map((seg) => {
            const color = segmentColor(bi, seg.seriesIndex)
            const name = resolvedData.series[seg.seriesIndex].name
            const d = seg.rounded
              ? roundedEndRectPath(seg.x, seg.y, seg.width, seg.height, CAP_RADIUS_PX, horizontal ? 'right' : 'top')
              : `M ${seg.x} ${seg.y} h ${seg.width} v ${seg.height} h ${-seg.width} Z`
            const dimmed = selected !== null && !sameBarHit(selected, { categoryIndex: bi, seriesIndex: seg.seriesIndex })
            return (
              <path
                key={seg.seriesIndex}
                d={d}
                data-bar-hit
                fill={color}
                className="cursor-pointer transition-opacity hover:opacity-85"
                style={dimmed ? { opacity: DIM_FILL } : undefined}
                onClick={onSegmentClick ? () => onSegmentClick({ categoryIndex: bi, seriesIndex: seg.seriesIndex }) : undefined}
                onPointerMove={(e) =>
                  setHover({
                    category: bar.category,
                    seriesName: name,
                    color,
                    value: seg.value,
                    total: bar.total,
                    x: e.clientX,
                    y: e.clientY,
                  })
                }
                onPointerLeave={() => setHover(null)}
              />
            )
          })}
          {bar.segments
            .filter((seg) => seg.labelFits)
            .map((seg) => {
              const dimmed = selected !== null && !sameBarHit(selected, { categoryIndex: bi, seriesIndex: seg.seriesIndex })
              return (
                <text
                  key={seg.seriesIndex}
                  x={seg.x + seg.width / 2}
                  y={seg.y + seg.height / 2}
                  fill={contrastingTextColor(segmentColor(bi, seg.seriesIndex))}
                  fontSize={10.5}
                  fontWeight={600}
                  opacity={dimmed ? DIM_FILL : 0.9}
                  textAnchor="middle"
                  dominantBaseline="middle"
                  className="pointer-events-none select-none"
                >
                  {formatValueWithUnit(seg.value, resolvedUnit)}
                </text>
              )
            })}
          {bar.totalLabel && (
            <text
              x={bar.totalLabel.x}
              y={bar.totalLabel.y}
              fill="#9ca3af"
              fontSize={10.5}
              textAnchor={horizontal ? 'start' : 'middle'}
              dominantBaseline={horizontal ? 'middle' : undefined}
              className="pointer-events-none select-none"
            >
              {bar.totalLabel.text}
            </text>
          )}
        </g>
      ))
    : null

  const lastTick = layout ? layout.valueTicks.length - 1 : -1

  return (
    <>
      {/* Body — plot (scrolls along its long axis) + side legend */}
      <div className="flex-1 min-h-0 flex gap-5 p-4 overflow-hidden">
        <div
          ref={plotRef}
          className={`flex-1 min-w-0 h-full ${horizontal ? 'overflow-y-auto overflow-x-hidden' : 'overflow-x-auto'}`}
        >
          {layout && horizontal && (
            <>
              <svg
                width={layout.contentWidth}
                height={AXIS_HEIGHT}
                viewBox={`0 0 ${layout.contentWidth} ${AXIS_HEIGHT}`}
                className="sticky top-0 z-10 block bg-app-panel"
                data-testid="bar-axis"
                onClick={handleBackgroundClick}
              >
                <line
                  x1={layout.labelColumnWidth}
                  x2={layout.contentWidth - RIGHT_MARGIN_PX}
                  y1={AXIS_HEIGHT - 0.5}
                  y2={AXIS_HEIGHT - 0.5}
                  stroke="#3a3a52"
                  strokeWidth={1}
                />
                {layout.valueTicks.map((tick, i) => (
                  <text
                    key={i}
                    x={tick.pos}
                    y={AXIS_HEIGHT - 8}
                    fill="#6b7280"
                    fontSize={11}
                    textAnchor={i === lastTick ? 'end' : 'middle'}
                  >
                    {tick.label}
                  </text>
                ))}
              </svg>
              <svg
                width={layout.contentWidth}
                height={layout.contentHeight}
                viewBox={`0 0 ${layout.contentWidth} ${layout.contentHeight}`}
                className="block"
                data-testid="bar-body"
                onClick={handleBackgroundClick}
                onMouseLeave={() => setHover(null)}
              >
                {hitRects}
                {layout.valueTicks.map((tick, i) => (
                  <line
                    key={i}
                    x1={tick.pos}
                    x2={tick.pos}
                    y1={0}
                    y2={layout.contentHeight}
                    stroke={tick.value === 0 ? '#3a3a52' : '#1f1f2e'}
                    strokeWidth={1}
                    className="pointer-events-none"
                  />
                ))}
                {layout.bars.map((bar, bi) => (
                  <text
                    key={bi}
                    x={layout.labelColumnWidth - 8}
                    y={bar.labelY}
                    fill="#9ca3af"
                    fontSize={11}
                    textAnchor="end"
                    dominantBaseline="middle"
                    {...labelHitProps(bi)}
                  >
                    <title>{bar.category}</title>
                    {bar.displayLabel}
                  </text>
                ))}
                {segments}
              </svg>
            </>
          )}
          {layout && !horizontal && (
            <svg
              width={layout.contentWidth}
              height={layout.contentHeight}
              viewBox={`0 0 ${layout.contentWidth} ${layout.contentHeight}`}
              onClick={handleBackgroundClick}
              onMouseLeave={() => setHover(null)}
            >
              {hitRects}
              {layout.valueTicks.map((tick, i) => (
                <g key={i} className="pointer-events-none">
                  <line
                    x1={layout.yAxisWidth}
                    x2={layout.contentWidth - RIGHT_MARGIN_PX}
                    y1={tick.pos}
                    y2={tick.pos}
                    stroke={tick.value === 0 ? '#3a3a52' : '#1f1f2e'}
                    strokeWidth={1}
                  />
                  <text
                    x={layout.yAxisWidth - 8}
                    y={tick.pos}
                    fill="#6b7280"
                    fontSize={11}
                    textAnchor="end"
                    dominantBaseline="middle"
                  >
                    {tick.label}
                  </text>
                </g>
              ))}
              {segments}
              {layout.bars.map((bar, bi) => (
                <text
                  key={bi}
                  x={bar.labelX}
                  y={layout.rotateLabels ? rotatedLabelY : layout.contentHeight - 8}
                  fill="#9ca3af"
                  fontSize={11}
                  textAnchor={layout.rotateLabels ? 'end' : 'middle'}
                  transform={layout.rotateLabels ? `rotate(${ROTATE_DEG} ${bar.labelX} ${rotatedLabelY})` : undefined}
                  {...labelHitProps(bi)}
                >
                  <title>{bar.category}</title>
                  {bar.displayLabel}
                </text>
              ))}
            </svg>
          )}
        </div>

        {!resolvedData.singleSeries && (
          <div className="min-w-[170px] h-full overflow-y-auto space-y-1.5">
            {resolvedData.series.map((s, i) => (
              <div key={i} className="flex items-center gap-2 px-1.5 py-0.5 rounded-sm">
                <div className="w-2.5 h-2.5 rounded-xs shrink-0" style={{ background: s.color }} />
                <span className="flex-1 min-w-0 truncate text-xs text-theme-text-secondary" title={s.name}>
                  {s.name}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>

      {hover && (
        <div
          className="fixed z-50 px-3 py-2 text-xs rounded-md shadow-lg pointer-events-none"
          style={{
            left: Math.min(hover.x + 14, window.innerWidth - 220),
            top: Math.min(hover.y - 10, window.innerHeight - 96),
            background: 'var(--app-bg)',
            border: '1px solid var(--border-color)',
            color: 'var(--text-primary)',
          }}
        >
          <div className="text-theme-text-muted mb-0.5">{hover.category}</div>
          <div className="flex items-center gap-2">
            <div className="w-2.5 h-2.5 rounded-xs shrink-0" style={{ background: hover.color }} />
            <span>{hover.seriesName}</span>
            <span className="font-semibold ml-auto">{formatValueWithUnit(hover.value, resolvedUnit)}</span>
          </div>
          {!resolvedData.singleSeries && (
            <>
              <div className="mt-1 pt-1 border-t border-theme-border flex text-theme-text-muted">
                <span>share of bar</span>
                <span className="ml-auto text-theme-text-secondary">
                  {hover.total > 0 ? ((100 * hover.value) / hover.total).toFixed(1) : '0.0'}%
                </span>
              </div>
              <div className="flex text-theme-text-muted">
                <span>bar total</span>
                <span className="ml-auto text-theme-text-secondary">{formatValueWithUnit(hover.total, resolvedUnit)}</span>
              </div>
            </>
          )}
        </div>
      )}
    </>
  )
}

export function StackedBarCell({
  data,
  status,
  options,
  variables,
  timeRange,
  cellResults,
  cellSelections,
  selectionMode,
  onSelectionChange,
}: CellRendererProps) {
  const table = data[0]

  const extraction = useMemo(() => {
    if (!table || table.numRows === 0) return null
    return extractStackedBarData(table)
  }, [table])

  const rawUnit = (options?.unit as string | undefined) ?? ''
  const resolvedUnit = useMemo(
    () => (rawUnit ? substituteMacros(rawUnit, variables, timeRange, cellResults, cellSelections) : ''),
    [rawUnit, variables, timeRange, cellResults, cellSelections],
  )

  const orientation: BarOrientation = options?.orientation === 'horizontal' ? 'horizontal' : 'vertical'
  const valueLabels = options?.valueLabels === true
  const selectable = selectionMode === 'single'

  const resolvedData: ResolvedBarData | null = useMemo(() => {
    if (!extraction || !extraction.ok) return null
    return { ...extraction.data, series: resolveSeriesColors(extraction.data.series) }
  }, [extraction])

  const maxTotal = useMemo(() => {
    if (!resolvedData) return 0
    return Math.max(0, ...resolvedData.categories.map((_, c) => resolvedData.values[c].reduce((a, b) => a + b, 0)))
  }, [resolvedData])

  const [selected, setSelected] = useState<BarHit | null>(null)

  // Cleared during render so the view never draws a stale hit against new data.
  const [dataForSelection, setDataForSelection] = useState(data)
  if (dataForSelection !== data) {
    setDataForSelection(data)
    setSelected(null)
  }

  // onSelectionChange is an inline arrow in NotebookRenderer; a ref keeps it out of effect deps.
  const onSelectionChangeRef = useLatestRef(onSelectionChange)

  // Published after commit: calling it during render would update a parent while rendering.
  useEffect(() => {
    onSelectionChangeRef.current?.(null)
  }, [data, onSelectionChangeRef])

  const handleSegmentClick = useCallback(
    (hit: BarHit) => {
      if (!resolvedData) return
      const next = sameBarHit(selected, hit) ? null : hit
      setSelected(next)
      onSelectionChangeRef.current?.(next ? buildBarSelection(resolvedData, next) : null)
    },
    [resolvedData, selected, onSelectionChangeRef],
  )

  const handleBackgroundClick = useCallback(() => {
    if (selected === null) return
    setSelected(null)
    onSelectionChangeRef.current?.(null)
  }, [selected, onSelectionChangeRef])

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center h-[200px]">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
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

  if (!extraction || !extraction.ok) {
    return (
      <div className="flex items-center justify-center h-[200px] text-accent-error text-sm">
        {extraction?.error ?? 'No data available'}
      </div>
    )
  }

  if (!resolvedData || resolvedData.categories.length === 0) {
    return (
      <div className="flex items-center justify-center h-[200px] text-theme-text-muted text-sm">
        No data available
      </div>
    )
  }

  // Every present value is 0: distinct from having no categories at all (handled
  // above). Every segment would have zero height, so show the same empty state
  // instead of a blank plot.
  if (maxTotal === 0) {
    return (
      <div className="flex items-center justify-center h-[200px] text-theme-text-muted text-sm">
        No data available
      </div>
    )
  }

  return (
    <div className="flex flex-col h-full bg-app-panel border border-theme-border rounded-lg">
      {/* Header — stats row, mirrors Pie/XY */}
      <div className="flex justify-between items-center px-4 py-3 border-b border-theme-border" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center gap-4 text-xs text-theme-text-muted">
          <div>
            categories: <span className="text-theme-text-secondary">{resolvedData.categories.length.toLocaleString()}</span>
          </div>
          {!resolvedData.singleSeries && (
            <div>
              series: <span className="text-theme-text-secondary">{resolvedData.series.length.toLocaleString()}</span>
            </div>
          )}
          <div>
            max total: <span className="text-theme-text-secondary">{formatValueWithUnit(maxTotal, resolvedUnit)}</span>
          </div>
        </div>
      </div>

      <StackedBarView
        resolvedData={resolvedData}
        resolvedUnit={resolvedUnit}
        maxTotal={maxTotal}
        orientation={orientation}
        valueLabels={valueLabels}
        selected={selectable ? selected : null}
        onSegmentClick={selectable ? handleSegmentClick : undefined}
        onBackgroundClick={selectable ? handleBackgroundClick : undefined}
      />
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function StackedBarCellEditor({ config, onChange, variables, timeRange, onRun, cellResults, cellSelections }: CellEditorProps) {
  const stackedBarConfig = config as QueryCellConfig

  const updateOption = useCallback(
    (key: string, value: unknown) => {
      onChange({ ...stackedBarConfig, options: { ...stackedBarConfig.options, [key]: value } })
    },
    [stackedBarConfig, onChange],
  )

  const validationErrors = useMemo(() => {
    const errors: string[] = []
    validateMacros(stackedBarConfig.sql, variables, cellResults, cellSelections).errors.forEach((e) => errors.push(e))
    const unit = stackedBarConfig.options?.unit as string | undefined
    if (unit) {
      validateMacros(unit, variables, cellResults, cellSelections).errors.forEach((e) => errors.push(`Unit: ${e}`))
    }
    return errors
  }, [stackedBarConfig.sql, stackedBarConfig.options?.unit, variables, cellResults, cellSelections])

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={stackedBarConfig.sql}
          onChange={(sql) => onChange({ ...stackedBarConfig, sql })}
          language="sql"
          placeholder="SELECT category, [series,] value FROM ..."
          minHeight="240px"
          onRunShortcut={onRun}
        />
        <p className="mt-1 text-[11px] text-theme-text-muted leading-snug">
          Return category (one bar each) and value (numeric) for a plain bar chart, or category,
          series (one stack segment each, shared legend), then value for a stacked one. Add a{' '}
          <code className="font-mono">color</code> column to color explicitly — it colors each bar
          with two columns and each series with three; the first non-null value seen wins.
          Duplicate (category, series) rows are summed. To stack to 100% instead, normalize in SQL
          with a window function and set the unit below to{' '}
          <code className="font-mono">percent</code>, e.g.{' '}
          <code className="font-mono">100.0 * value / sum(value) OVER (PARTITION BY category)</code>.{' '}
          <a href={QUERY_GUIDE_URL} target="_blank" rel="noreferrer" className="text-accent-link hover:underline">
            Functions reference
          </a>
        </p>
      </div>

      {validationErrors.length > 0 && (
        <div className="text-red-400 text-sm space-y-1">
          {validationErrors.map((err, i) => (
            <div key={i}>⚠ {err}</div>
          ))}
        </div>
      )}

      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Unit
        </label>
        <input
          type="text"
          value={(stackedBarConfig.options?.unit as string | undefined) ?? ''}
          onChange={(e) => updateOption('unit', e.target.value)}
          className="w-full px-3 py-1.5 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-xs focus:outline-hidden focus:border-accent-link"
          placeholder="e.g., count, bytes, ms, percent"
        />
      </div>

      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Orientation
        </label>
        <select
          value={stackedBarConfig.options?.orientation === 'horizontal' ? 'horizontal' : 'vertical'}
          onChange={(e) => updateOption('orientation', e.target.value)}
          className="w-full px-3 py-1.5 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-xs focus:outline-hidden focus:border-accent-link"
        >
          <option value="vertical">Vertical</option>
          <option value="horizontal">Horizontal</option>
        </select>
      </div>

      <label className="flex items-center gap-2 text-xs text-theme-text-secondary cursor-pointer">
        <input
          type="checkbox"
          checked={stackedBarConfig.options?.valueLabels === true}
          onChange={(e) => updateOption('valueLabels', e.target.checked)}
        />
        Show value labels
      </label>

      <RowSelectionEditor
        selectionMode={(stackedBarConfig.options?.selectionMode as 'none' | 'single' | undefined) ?? 'none'}
        cellName={stackedBarConfig.name}
        onChange={(mode) => updateOption('selectionMode', mode)}
      />

      <AvailableVariablesPanel variables={variables} timeRange={timeRange} cellResults={cellResults} cellSelections={cellSelections} />
      <DocumentationLink url={QUERY_GUIDE_URL} label="Query Guide" />
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

// eslint-disable-next-line react-refresh/only-export-components
export const stackedBarMetadata: CellTypeMetadata = {
  renderer: StackedBarCell,
  EditorComponent: StackedBarCellEditor,

  label: 'Bar Chart',
  icon: <ChartColumnStacked />,
  description: 'Vertical or horizontal bars, one per category, optionally stacked by series',
  showTypeBadge: true,
  defaultHeight: 360,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'stackedbar' as const,
    sql: DEFAULT_SQL.stackedbar,
    options: {},
  }),

  execute: async (
    config: CellConfig,
    { variables, cellResults, cellSelections, timeRange, runQuery, runQueryAs }: CellExecutionContext,
  ) => {
    const stackedBarConfig = config as QueryCellConfig
    const sql = substituteMacros(stackedBarConfig.sql, variables, timeRange, cellResults, cellSelections)
    if (runQueryAs) {
      const table = await runQueryAs(sql, config.name, stackedBarConfig.dataSource)
      return { data: [table] }
    }
    const table = await runQuery(sql)
    return { data: [table] }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
    options: { ...(config as QueryCellConfig).options },
  }),
}
