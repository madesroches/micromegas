import { useCallback, useMemo, useState } from 'react'
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
import { extractPieData, type PieSlice } from '@/lib/arrow-utils'
import { formatValueWithUnit } from '@/lib/format-value'
import { SERIES_COLORS } from '@/components/chart-constants'
import { PieChart as PieChartIcon } from 'lucide-react'

export type PieChartType = 'pie' | 'donut'

const DEFAULT_MAX_SLICES = 8

/** Fixed muted-gray fill for the synthetic "Other" slice — never the rotating palette. */
const OTHER_SLICE_COLOR = 'var(--text-muted)'
/** Stroke color between touching slices ("surface gap"), matching the panel background. */
const SURFACE_GAP_COLOR = 'var(--panel-bg)'

// =============================================================================
// "Other" grouping (client-side, top-N + fold-the-rest)
// =============================================================================

export interface ResolvedPieSlice {
  label: string
  value: number
  color: string
  /** Number of raw categories folded together — set only on the synthetic "Other" slice. */
  foldedCount?: number
}

/**
 * Sort slices descending by value; when there are more than `maxSlices`,
 * keep the top `maxSlices - 1` and fold the remainder into one "Other" slice
 * whose value is the sum of the tail. Colors: each visible slice keeps its
 * SQL-supplied color if present, otherwise the next unused `SERIES_COLORS`
 * entry in fixed order; "Other" always gets the fixed muted-gray color.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function groupPieSlices(slices: PieSlice[], maxSlices: number): ResolvedPieSlice[] {
  const n = Number.isFinite(maxSlices) && maxSlices >= 1 ? Math.floor(maxSlices) : DEFAULT_MAX_SLICES
  const sorted = [...slices].sort((a, b) => b.value - a.value)

  const visible = sorted.length > n ? sorted.slice(0, n - 1) : sorted
  const folded = sorted.length > n ? sorted.slice(n - 1) : []

  let paletteIdx = 0
  const resolved: ResolvedPieSlice[] = visible.map((s) => {
    if (s.color) {
      return { label: s.label, value: s.value, color: s.color }
    }
    const color = SERIES_COLORS[paletteIdx % SERIES_COLORS.length]
    paletteIdx++
    return { label: s.label, value: s.value, color }
  })

  if (folded.length > 0) {
    resolved.push({
      label: 'Other',
      value: folded.reduce((sum, s) => sum + s.value, 0),
      color: OTHER_SLICE_COLOR,
      foldedCount: folded.length,
    })
  }

  return resolved
}

// =============================================================================
// Arc math — pie (rInner=0) and donut (rInner>0) share the same path builder
// =============================================================================

function polarToCartesian(cx: number, cy: number, r: number, angleRad: number): [number, number] {
  return [cx + r * Math.cos(angleRad), cy + r * Math.sin(angleRad)]
}

/** Tolerance for treating a slice's angular span as a full circle (float error near 2π). */
const FULL_CIRCLE_EPSILON = 1e-9

/**
 * Full-circle path built from two 180° arcs back-to-back. A single arc spanning
 * a full 360° is degenerate — its start/end points coincide and SVG collapses it
 * to nothing — so the circle must be split into two halves instead.
 */
function fullCirclePath(cx: number, cy: number, r: number): string {
  return [
    `M ${cx + r} ${cy}`,
    `A ${r} ${r} 0 1 1 ${cx - r} ${cy}`,
    `A ${r} ${r} 0 1 1 ${cx + r} ${cy}`,
    'Z',
  ].join(' ')
}

/** Builds an SVG path for one slice: `M/L/A/Z` for a pie wedge, `M/A/L/A/Z` for a donut ring segment. */
function slicePath(
  cx: number,
  cy: number,
  rOuter: number,
  rInner: number,
  startAngle: number,
  endAngle: number,
): string {
  // A 100%-share slice spans (near) the full circle, which is degenerate for the arc
  // math below (start/end points coincide). Render a full ring instead: a solid disc
  // for the pie case, or an outer+inner circle pair (drawn with fill-rule evenodd) for
  // the donut case.
  if (endAngle - startAngle >= 2 * Math.PI - FULL_CIRCLE_EPSILON) {
    return rInner <= 0
      ? fullCirclePath(cx, cy, rOuter)
      : `${fullCirclePath(cx, cy, rOuter)} ${fullCirclePath(cx, cy, rInner)}`
  }

  const large = endAngle - startAngle > Math.PI ? 1 : 0
  const [x1, y1] = polarToCartesian(cx, cy, rOuter, startAngle)
  const [x2, y2] = polarToCartesian(cx, cy, rOuter, endAngle)

  if (rInner <= 0) {
    return [
      `M ${cx} ${cy}`,
      `L ${x1} ${y1}`,
      `A ${rOuter} ${rOuter} 0 ${large} 1 ${x2} ${y2}`,
      'Z',
    ].join(' ')
  }

  const [x3, y3] = polarToCartesian(cx, cy, rInner, endAngle)
  const [x4, y4] = polarToCartesian(cx, cy, rInner, startAngle)
  return [
    `M ${x1} ${y1}`,
    `A ${rOuter} ${rOuter} 0 ${large} 1 ${x2} ${y2}`,
    `L ${x3} ${y3}`,
    `A ${rInner} ${rInner} 0 ${large} 0 ${x4} ${y4}`,
    'Z',
  ].join(' ')
}

interface SliceGeometry {
  slice: ResolvedPieSlice
  path: string
  fraction: number
  /** Midpoint angle (radians), for direct-label placement. */
  midAngle: number
}

const CHART_SIZE = 200
const CENTER = CHART_SIZE / 2
const R_OUTER = 92
const R_INNER_DONUT = 56
/** Slices at or above this share get a direct percentage label. */
const DIRECT_LABEL_THRESHOLD = 0.08

// eslint-disable-next-line react-refresh/only-export-components
export function buildSliceGeometry(slices: ResolvedPieSlice[], total: number, chartType: PieChartType): SliceGeometry[] {
  const rInner = chartType === 'donut' ? R_INNER_DONUT : 0
  let angle = -Math.PI / 2 // start at 12 o'clock
  const geometry: SliceGeometry[] = []
  for (const slice of slices) {
    const fraction = total > 0 ? slice.value / total : 0
    const start = angle
    const end = angle + fraction * 2 * Math.PI
    angle = end
    geometry.push({
      slice,
      path: slicePath(CENTER, CENTER, R_OUTER, rInner, start, end),
      fraction,
      midAngle: (start + end) / 2,
    })
  }
  return geometry
}

// =============================================================================
// Renderer Component
// =============================================================================

interface HoverState {
  slice: ResolvedPieSlice
  fraction: number
  x: number
  y: number
}

export function PieChartCell({
  data,
  status,
  options,
  onOptionsChange,
  variables,
  timeRange,
  cellResults,
  cellSelections,
}: CellRendererProps) {
  const table = data[0]

  const pieResult = useMemo(() => {
    if (!table || table.numRows === 0) return null
    return extractPieData(table)
  }, [table])

  const rawUnit = (options?.unit as string | undefined) ?? ''
  const resolvedUnit = useMemo(
    () => (rawUnit ? substituteMacros(rawUnit, variables, timeRange, cellResults, cellSelections) : ''),
    [rawUnit, variables, timeRange, cellResults, cellSelections],
  )

  const chartType: PieChartType = (options?.chart_type as PieChartType) === 'pie' ? 'pie' : 'donut'
  const maxSlices =
    typeof options?.max_slices === 'number' && options.max_slices >= 1 ? options.max_slices : DEFAULT_MAX_SLICES

  const resolvedSlices = useMemo(() => {
    if (!pieResult || !pieResult.ok) return []
    return groupPieSlices(pieResult.slices, maxSlices)
  }, [pieResult, maxSlices])

  const total = useMemo(() => resolvedSlices.reduce((sum, s) => sum + s.value, 0), [resolvedSlices])

  const geometry = useMemo(
    () => buildSliceGeometry(resolvedSlices, total, chartType),
    [resolvedSlices, total, chartType],
  )

  const handleChartTypeChange = useCallback(
    (type: PieChartType) => {
      onOptionsChange({ ...options, chart_type: type })
    },
    [options, onOptionsChange],
  )

  const [hover, setHover] = useState<HoverState | null>(null)

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

  if (!pieResult || !pieResult.ok) {
    return (
      <div className="flex items-center justify-center h-[200px] text-accent-error text-sm">
        {pieResult?.error ?? 'No data available'}
      </div>
    )
  }

  if (resolvedSlices.length === 0) {
    return (
      <div className="flex items-center justify-center h-[200px] text-theme-text-muted text-sm">
        No data available
      </div>
    )
  }

  return (
    <div className="flex flex-col h-full bg-app-panel border border-theme-border rounded-lg">
      {/* Header — mirrors XYChart's stats row + chart-type toggle */}
      <div className="flex justify-between items-center px-4 py-3 border-b border-theme-border" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center gap-4 text-xs text-theme-text-muted">
          <div>
            categories: <span className="text-theme-text-secondary">{resolvedSlices.length.toLocaleString()}</span>
          </div>
          <div>
            total: <span className="text-theme-text-secondary">{formatValueWithUnit(total, resolvedUnit)}</span>
          </div>
        </div>
        <div className="flex border border-theme-border rounded-sm overflow-hidden" title="Chart display style">
          <button
            onClick={() => handleChartTypeChange('donut')}
            className={`px-2 py-0.5 text-[11px] transition-colors ${
              chartType === 'donut'
                ? 'bg-accent text-white'
                : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-white/5'
            }`}
          >
            Donut
          </button>
          <button
            onClick={() => handleChartTypeChange('pie')}
            className={`px-2 py-0.5 text-[11px] border-l border-theme-border transition-colors ${
              chartType === 'pie'
                ? 'bg-accent text-white'
                : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-white/5'
            }`}
          >
            Pie
          </button>
        </div>
      </div>

      {/* Body — donut/pie SVG + side legend */}
      <div className="flex-1 min-h-0 flex items-center gap-7 p-5 overflow-hidden">
        <div className="relative shrink-0" style={{ width: CHART_SIZE, height: CHART_SIZE }}>
          <svg
            viewBox={`0 0 ${CHART_SIZE} ${CHART_SIZE}`}
            width={CHART_SIZE}
            height={CHART_SIZE}
            onMouseLeave={() => setHover(null)}
          >
            {geometry.map((g, i) => (
              <path
                key={i}
                d={g.path}
                fill={g.slice.color}
                fillRule="evenodd"
                stroke={SURFACE_GAP_COLOR}
                strokeWidth={2}
                className="cursor-pointer transition-opacity hover:opacity-85"
                onPointerMove={(e) =>
                  setHover({ slice: g.slice, fraction: g.fraction, x: e.clientX, y: e.clientY })
                }
                onPointerLeave={() => setHover(null)}
              />
            ))}
            {geometry
              .filter((g) => g.fraction >= DIRECT_LABEL_THRESHOLD)
              .map((g, i) => {
                const labelR = chartType === 'donut' ? (R_OUTER + R_INNER_DONUT) / 2 : R_OUTER * 0.62
                const [lx, ly] = polarToCartesian(CENTER, CENTER, labelR, g.midAngle)
                return (
                  <text
                    key={i}
                    x={lx}
                    y={ly}
                    textAnchor="middle"
                    dominantBaseline="middle"
                    fontSize={11}
                    fontWeight={600}
                    fill="#0a0a0f"
                    opacity={0.85}
                    className="pointer-events-none select-none"
                  >
                    {Math.round(g.fraction * 100)}%
                  </text>
                )
              })}
          </svg>
          {chartType === 'donut' && (
            <div className="absolute inset-0 flex flex-col items-center justify-center pointer-events-none text-center">
              <div className="text-xl font-semibold text-theme-text-primary">
                {formatValueWithUnit(total, resolvedUnit)}
              </div>
              <div className="text-[11px] text-theme-text-muted mt-0.5">total</div>
            </div>
          )}
        </div>

        <div className="flex-1 min-w-0 h-full overflow-y-auto space-y-1.5">
          {geometry.map((g, i) => (
            <div key={i} className="flex items-center gap-2 px-1.5 py-0.5 rounded-sm">
              <div className="w-2.5 h-2.5 rounded-xs shrink-0" style={{ background: g.slice.color }} />
              <span className="flex-1 min-w-0 truncate text-xs text-theme-text-secondary" title={g.slice.label}>
                {g.slice.label}
                {g.slice.foldedCount != null && (
                  <span className="text-theme-text-muted"> ({g.slice.foldedCount})</span>
                )}
              </span>
              <span className="text-xs font-semibold text-theme-text-primary tabular-nums">
                {formatValueWithUnit(g.slice.value, resolvedUnit)}
              </span>
              <span className="w-12 text-right text-[11px] text-theme-text-muted tabular-nums">
                {(g.fraction * 100).toFixed(1)}%
              </span>
            </div>
          ))}
        </div>
      </div>

      {hover && (
        <div
          className="fixed z-10 px-3 py-2 text-xs rounded-md shadow-lg pointer-events-none"
          style={{
            left: hover.x + 14,
            top: hover.y - 10,
            background: 'var(--app-bg)',
            border: '1px solid var(--border-color)',
            color: 'var(--text-primary)',
          }}
        >
          <div className="text-theme-text-muted mb-0.5">
            {hover.slice.label}
            {hover.slice.foldedCount != null && ` (${hover.slice.foldedCount} categories)`}
          </div>
          <div className="font-semibold">
            {formatValueWithUnit(hover.slice.value, resolvedUnit)} ({(hover.fraction * 100).toFixed(1)}%)
          </div>
        </div>
      )}
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function PieChartCellEditor({
  config,
  onChange,
  variables,
  timeRange,
  onRun,
  cellResults,
  cellSelections,
}: CellEditorProps) {
  const pieConfig = config as QueryCellConfig

  const updateOption = useCallback(
    (key: string, value: unknown) => {
      onChange({ ...pieConfig, options: { ...pieConfig.options, [key]: value } })
    },
    [pieConfig, onChange],
  )

  const chartType: PieChartType = (pieConfig.options?.chart_type as PieChartType) === 'pie' ? 'pie' : 'donut'
  const maxSlices =
    typeof pieConfig.options?.max_slices === 'number' ? pieConfig.options.max_slices : DEFAULT_MAX_SLICES

  const validationErrors = useMemo(() => {
    const errors: string[] = []
    validateMacros(pieConfig.sql, variables, cellResults, cellSelections).errors.forEach((e) => errors.push(e))
    const unit = pieConfig.options?.unit as string | undefined
    if (unit) {
      validateMacros(unit, variables, cellResults, cellSelections).errors.forEach((e) => errors.push(`Unit: ${e}`))
    }
    return errors
  }, [pieConfig.sql, pieConfig.options?.unit, variables, cellResults, cellSelections])

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={pieConfig.sql}
          onChange={(sql) => onChange({ ...pieConfig, sql })}
          language="sql"
          placeholder="SELECT category, value FROM ..."
          minHeight="240px"
          onRunShortcut={onRun}
        />
        <p className="mt-1 text-[11px] text-theme-text-muted leading-snug">
          Query must return exactly two columns: category (string) then value (numeric). Add a{' '}
          <code className="font-mono">color</code> column (packed RGBA u32, e.g. via{' '}
          <code className="font-mono">rgba()</code> or <code className="font-mono">color_scale()</code>) to color
          each slice explicitly — otherwise slices are colored from the default palette in value order.{' '}
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

      <div className="flex gap-3">
        <div className="flex-1">
          <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
            Unit
          </label>
          <input
            type="text"
            value={(pieConfig.options?.unit as string | undefined) ?? ''}
            onChange={(e) => updateOption('unit', e.target.value)}
            className="w-full px-3 py-1.5 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-xs focus:outline-hidden focus:border-accent-link"
            placeholder="e.g., count, bytes, ms"
          />
        </div>
        <div className="flex-1">
          <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
            Max Slices
          </label>
          <input
            type="number"
            min={1}
            value={maxSlices}
            onChange={(e) => {
              const n = parseInt(e.target.value, 10)
              updateOption('max_slices', Number.isFinite(n) && n >= 1 ? n : DEFAULT_MAX_SLICES)
            }}
            className="w-full px-3 py-1.5 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-xs focus:outline-hidden focus:border-accent-link"
          />
        </div>
      </div>

      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Chart Type
        </label>
        <div className="flex border border-theme-border rounded-sm overflow-hidden w-fit">
          <button
            type="button"
            onClick={() => updateOption('chart_type', 'donut')}
            className={`px-3 py-1 text-xs transition-colors ${
              chartType === 'donut'
                ? 'bg-accent text-white'
                : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-white/5'
            }`}
          >
            Donut
          </button>
          <button
            type="button"
            onClick={() => updateOption('chart_type', 'pie')}
            className={`px-3 py-1 text-xs border-l border-theme-border transition-colors ${
              chartType === 'pie'
                ? 'bg-accent text-white'
                : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-white/5'
            }`}
          >
            Pie
          </button>
        </div>
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
export const pieChartMetadata: CellTypeMetadata = {
  renderer: PieChartCell,
  EditorComponent: PieChartCellEditor,

  label: 'Pie Chart',
  icon: <PieChartIcon />,
  description: 'Proportions/breakdown chart (pie or donut)',
  showTypeBadge: true,
  defaultHeight: 320,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'piechart' as const,
    sql: DEFAULT_SQL.piechart,
    options: {
      chart_type: 'donut' as const,
      max_slices: DEFAULT_MAX_SLICES,
    },
  }),

  execute: async (
    config: CellConfig,
    { variables, cellResults, cellSelections, timeRange, runQuery, runQueryAs }: CellExecutionContext,
  ) => {
    const pieConfig = config as QueryCellConfig
    const sql = substituteMacros(pieConfig.sql, variables, timeRange, cellResults, cellSelections)
    if (runQueryAs) {
      const table = await runQueryAs(sql, config.name, pieConfig.dataSource)
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
