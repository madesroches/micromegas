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
  /** Only the topmost non-zero segment of a bar is rounded; the baseline stays square. */
  rounded: boolean
  /** Whether the in-segment value label fits (18px min height, text width vs. bar width). */
  labelFits: boolean
}

export interface StackedBarBarLayout {
  category: string
  /** `category`, truncated with an ellipsis when rotated labels would otherwise overrun the label area; equal to `category` otherwise. */
  displayLabel: string
  x: number
  width: number
  total: number
  labelX: number
  segments: StackedBarSegmentLayout[]
}

export interface StackedBarYTick {
  value: number
  y: number
  label: string
}

export interface StackedBarLayout {
  bars: StackedBarBarLayout[]
  yTicks: StackedBarYTick[]
  yAxisWidth: number
  /** Total content width the plot must render at; exceeds `width` when bars would otherwise go below the floor, triggering horizontal scroll. */
  plotWidth: number
  plotHeight: number
  barWidth: number
  rotateLabels: boolean
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
 * Bar/segment rectangles, y ticks, and x-label placement for the stacked bar plot.
 * Mirrors XYChart's y-scale (max x 1.05, free ticks — no snapping/epsilon) and the
 * Pie cell's mark spec (surface-gap stroke, rounded cap only on the outermost mark).
 */
// eslint-disable-next-line react-refresh/only-export-components
export function buildStackedBarLayout(
  resolved: { categories: string[]; series: ResolvedStackedBarSeries[]; values: number[][] },
  opts: { width: number; height: number; unit: string },
): StackedBarLayout {
  const { categories, series, values } = resolved
  const { width, height, unit } = opts

  const totals = categories.map((_, c) => values[c].reduce((a, b) => a + b, 0))
  const maxTotal = Math.max(0, ...totals)
  const top = maxTotal > 0 ? maxTotal * 1.05 : 1
  const step = niceStep(top / Y_TICK_DIVISIONS)

  const tickCount = Math.floor(top / step + 1e-9) + 1
  const tickLabels: string[] = []
  for (let i = 0; i < tickCount; i++) {
    tickLabels.push(formatValueWithUnit(step * i, unit))
  }
  const maxTickLabelWidth = Math.max(0, ...tickLabels.map(estimateLabelWidth))
  const yAxisWidth = Math.ceil(maxTickLabelWidth) + Y_AXIS_CHROME_PX

  // Rotate when a label would overlap its neighbor at the *natural* (pre-floor) band width.
  const naturalPlotWidth = Math.max(0, width - yAxisWidth - RIGHT_MARGIN_PX)
  const naturalBand = categories.length > 0 ? naturalPlotWidth / categories.length : 0
  const maxCategoryLabelWidth = Math.max(0, ...categories.map(estimateLabelWidth))
  const rotateLabels = categories.length > 0 && maxCategoryLabelWidth > naturalBand

  const band = Math.max(naturalBand, MIN_BAND_WIDTH)
  const barWidth = Math.min(BAR_MAX_WIDTH, band * BAR_WIDTH_FRACTION)
  const plotWidth = Math.max(width, yAxisWidth + RIGHT_MARGIN_PX + band * categories.length)

  const labelAreaHeight = rotateLabels ? X_LABEL_AREA_HEIGHT_ROTATED : X_LABEL_AREA_HEIGHT
  const plotHeight = Math.max(0, height - TOP_MARGIN_PX - labelAreaHeight)

  const yForValue = (v: number) => TOP_MARGIN_PX + plotHeight - (v / top) * plotHeight

  const yTicks: StackedBarYTick[] = tickLabels.map((label, i) => ({
    value: step * i,
    y: yForValue(step * i),
    label,
  }))

  const bars: StackedBarBarLayout[] = categories.map((category, c) => {
    const x = yAxisWidth + band * c + (band - barWidth) / 2
    const total = totals[c]
    const present = series
      .map((_, si) => ({ si, value: values[c][si] }))
      .filter((entry) => entry.value > 0)

    let acc = 0
    const segments: StackedBarSegmentLayout[] = present.map((entry, idx) => {
      const isTop = idx === present.length - 1
      const y0 = yForValue(acc)
      acc += entry.value
      const y1 = yForValue(acc)
      const gap = isTop ? 0 : SEGMENT_GAP_PX
      const height = Math.max(y0 - y1 - gap, 0)
      const y = y1 + gap
      const label = formatValueWithUnit(entry.value, unit)
      const labelFits = height >= MIN_LABEL_SEGMENT_HEIGHT && estimateLabelWidth(label) + LABEL_TEXT_PADDING <= barWidth
      return {
        seriesIndex: entry.si,
        value: entry.value,
        x,
        y,
        width: barWidth,
        height,
        rounded: isTop,
        labelFits,
      }
    })

    const displayLabel = rotateLabels ? truncateLabel(category, MAX_ROTATED_LABEL_WIDTH_PX) : category

    return { category, displayLabel, x, width: barWidth, total, labelX: x + barWidth / 2, segments }
  })

  return { bars, yTicks, yAxisWidth, plotWidth, plotHeight, barWidth, rotateLabels }
}

/** SVG path for a rect with rounded top corners and a square bottom — the stack's top cap. */
function roundedTopRectPath(x: number, y: number, width: number, height: number, radius: number): string {
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

interface StackedBarViewProps {
  resolvedData: StackedBarData & { series: ResolvedStackedBarSeries[] }
  resolvedUnit: string
  maxTotal: number
}

/**
 * Owns the measured plot area (ResizeObserver) and hover state. Mounted only once
 * data is ready to render (see `StackedBarCell`'s success-path return), so the
 * ResizeObserver attaches to an already-present div — mirrors `FlameGraphView` in
 * FlameGraphCell.tsx.
 */
function StackedBarView({ resolvedData, resolvedUnit, maxTotal }: StackedBarViewProps) {
  const plotRef = useRef<HTMLDivElement>(null)
  const [dimensions, setDimensions] = useState({ width: 0, height: 0 })

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
    if (dimensions.width === 0 || dimensions.height === 0) return null
    return buildStackedBarLayout(resolvedData, { width: dimensions.width, height: dimensions.height, unit: resolvedUnit })
  }, [resolvedData, maxTotal, dimensions, resolvedUnit])

  const [hover, setHover] = useState<HoverState | null>(null)

  // Anchor point for a rotated label: the top of the label area (just below the
  // plot), not the bottom of the SVG — end-anchored text rotated -45deg hangs
  // down-left from there, staying within X_LABEL_AREA_HEIGHT_ROTATED instead of
  // being clipped past the bottom edge.
  const rotatedLabelY = layout ? TOP_MARGIN_PX + layout.plotHeight + ROTATED_LABEL_GUTTER_PX : 0

  return (
    <>
      {/* Body — plot (scrolls horizontally when bars hit the width floor) + side legend */}
      <div className="flex-1 min-h-0 flex gap-5 p-4 overflow-hidden">
        <div ref={plotRef} className="flex-1 min-w-0 h-full overflow-x-auto">
          {layout && (
            <svg
              width={layout.plotWidth}
              height={dimensions.height}
              viewBox={`0 0 ${layout.plotWidth} ${dimensions.height}`}
              onMouseLeave={() => setHover(null)}
            >
              {layout.yTicks.map((tick, i) => (
                <g key={i}>
                  <line
                    x1={layout.yAxisWidth}
                    x2={layout.plotWidth - RIGHT_MARGIN_PX}
                    y1={tick.y}
                    y2={tick.y}
                    stroke={tick.value === 0 ? '#3a3a52' : '#1f1f2e'}
                    strokeWidth={1}
                  />
                  <text
                    x={layout.yAxisWidth - 8}
                    y={tick.y}
                    fill="#6b7280"
                    fontSize={11}
                    textAnchor="end"
                    dominantBaseline="middle"
                  >
                    {tick.label}
                  </text>
                </g>
              ))}

              {layout.bars.map((bar, bi) => (
                <g key={bi}>
                  {bar.segments.map((seg) => {
                    const s = resolvedData.series[seg.seriesIndex]
                    const d = seg.rounded
                      ? roundedTopRectPath(seg.x, seg.y, seg.width, seg.height, CAP_RADIUS_PX)
                      : `M ${seg.x} ${seg.y} h ${seg.width} v ${seg.height} h ${-seg.width} Z`
                    return (
                      <path
                        key={seg.seriesIndex}
                        d={d}
                        fill={s.color}
                        className="cursor-pointer transition-opacity hover:opacity-85"
                        onPointerMove={(e) =>
                          setHover({
                            category: bar.category,
                            seriesName: s.name,
                            color: s.color,
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
                      const s = resolvedData.series[seg.seriesIndex]
                      return (
                        <text
                          key={seg.seriesIndex}
                          x={seg.x + seg.width / 2}
                          y={seg.y + seg.height / 2}
                          fill={contrastingTextColor(s.color)}
                          fontSize={10.5}
                          fontWeight={600}
                          opacity={0.9}
                          textAnchor="middle"
                          dominantBaseline="middle"
                          className="pointer-events-none select-none"
                        >
                          {formatValueWithUnit(seg.value, resolvedUnit)}
                        </text>
                      )
                    })}
                  <text
                    x={bar.labelX}
                    y={layout.rotateLabels ? rotatedLabelY : dimensions.height - 8}
                    fill="#9ca3af"
                    fontSize={11}
                    textAnchor={layout.rotateLabels ? 'end' : 'middle'}
                    transform={layout.rotateLabels ? `rotate(${ROTATE_DEG} ${bar.labelX} ${rotatedLabelY})` : undefined}
                  >
                    <title>{bar.category}</title>
                    {bar.displayLabel}
                  </text>
                </g>
              ))}
            </svg>
          )}
        </div>

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
        </div>
      )}
    </>
  )
}

export function StackedBarCell({ data, status, options, variables, timeRange, cellResults, cellSelections }: CellRendererProps) {
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

  const resolvedSeries = useMemo(() => {
    if (!extraction || !extraction.ok) return []
    return resolveSeriesColors(extraction.data.series)
  }, [extraction])

  const resolvedData: (StackedBarData & { series: ResolvedStackedBarSeries[] }) | null = useMemo(() => {
    if (!extraction || !extraction.ok) return null
    return { categories: extraction.data.categories, series: resolvedSeries, values: extraction.data.values }
  }, [extraction, resolvedSeries])

  const maxTotal = useMemo(() => {
    if (!resolvedData) return 0
    return Math.max(0, ...resolvedData.categories.map((_, c) => resolvedData.values[c].reduce((a, b) => a + b, 0)))
  }, [resolvedData])

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
          <div>
            series: <span className="text-theme-text-secondary">{resolvedData.series.length.toLocaleString()}</span>
          </div>
          <div>
            max total: <span className="text-theme-text-secondary">{formatValueWithUnit(maxTotal, resolvedUnit)}</span>
          </div>
        </div>
      </div>

      <StackedBarView resolvedData={resolvedData} resolvedUnit={resolvedUnit} maxTotal={maxTotal} />
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
          placeholder="SELECT category, series, value FROM ..."
          minHeight="240px"
          onRunShortcut={onRun}
        />
        <p className="mt-1 text-[11px] text-theme-text-muted leading-snug">
          Query must return exactly three columns: category (one bar each), series (one stack
          segment each, shared legend), then value (numeric). Add a{' '}
          <code className="font-mono">color</code> column to color a series explicitly — the first
          non-null value seen for a series wins. Duplicate (category, series) rows are summed. To
          stack to 100% instead, normalize in SQL with a window function and set the unit below to{' '}
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

  label: 'Stacked Bar',
  icon: <ChartColumnStacked />,
  description: 'Breakdown of a value across stacked series, one bar per category',
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
