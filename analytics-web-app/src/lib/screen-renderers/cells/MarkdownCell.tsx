import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import type { Table } from 'apache-arrow'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import type { CellTypeMetadata, CellRendererProps, CellEditorProps, CellExecutionContext } from '../cell-registry'
import type { MarkdownCellConfig, CellConfig, CellState } from '../notebook-types'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { DocumentationLink, QUERY_GUIDE_URL } from '@/components/DocumentationLink'
import { TemplateWarningBanner } from '@/components/TemplateWarningBanner'
import { evaluateTemplate, validateMacros, validateTemplateMacros, substituteMacros, DEFAULT_SQL } from '../notebook-utils'
import { rowValues, columnTypeMap, resolveColorColumn } from '@/lib/arrow-utils'
import { cellColorToCss } from '@/lib/color-utils'
import { FileText } from 'lucide-react'

/** `config.sql` for a new/edited cell; falls back to `DEFAULT_SQL.markdown` for
 *  saved cells that predate query-backed markdown (no `sql`) and for a cleared editor. */
function effectiveMarkdownSql(config: MarkdownCellConfig): string {
  return config.sql?.trim() ? config.sql : DEFAULT_SQL.markdown
}

// =============================================================================
// Color resolution
// =============================================================================

function decodeColorColumn(
  table: Table,
  row: Record<string, unknown>,
  name: string,
): { css?: string; warning?: string } {
  const resolved = resolveColorColumn(table.schema.fields, name)
  if (resolved.index < 0) return {}
  if (resolved.error) return { warning: resolved.error }
  const value = row[resolved.name!]
  if (value === undefined) return {}
  const css = cellColorToCss(value, resolved.kind!)
  return css ? { css } : {}
}

/** Decodes the optional `color` / `background_color` columns of row 0 into CSS colors.
 *  A null or malformed value means no tint (not an error); an unsupported column type
 *  adds a warning instead of failing the cell — the text is still meaningful untinted. */
// eslint-disable-next-line react-refresh/only-export-components
export function resolveMarkdownColors(
  table: Table,
  row: Record<string, unknown>,
): { color?: string; backgroundColor?: string; warnings: string[] } {
  const color = decodeColorColumn(table, row, 'color')
  const background = decodeColorColumn(table, row, 'background_color')
  const warnings = [color.warning, background.warning].filter((w): w is string => !!w)
  return { color: color.css, backgroundColor: background.css, warnings }
}

// =============================================================================
// Prose classes
// =============================================================================

// Tailwind v4 only emits classes that appear literally in source, so each variant is
// spelled out in full below rather than built with `${}` interpolation of the color token.
const TINTED_PROSE_CLASSES =
  'prose prose-invert max-w-none prose-headings:text-inherit prose-p:text-inherit ' +
  'prose-a:text-accent-link prose-strong:text-inherit prose-em:text-inherit prose-li:text-inherit ' +
  'prose-blockquote:text-inherit prose-th:text-inherit prose-td:text-inherit marker:text-inherit ' +
  'prose-code:text-accent-highlight prose-code:bg-app-card prose-code:px-1 prose-code:py-0.5 ' +
  'prose-code:rounded-sm prose-code:before:content-none prose-code:after:content-none prose-pre:bg-app-card'

const UNTINTED_PROSE_CLASSES =
  'prose prose-invert max-w-none prose-headings:text-theme-text-primary prose-p:text-theme-text-secondary ' +
  'prose-a:text-accent-link prose-strong:text-theme-text-primary prose-em:text-theme-text-secondary ' +
  'prose-li:text-theme-text-secondary prose-blockquote:text-theme-text-secondary prose-th:text-theme-text-primary ' +
  'prose-td:text-theme-text-secondary marker:text-theme-text-muted ' +
  'prose-code:text-accent-highlight prose-code:bg-app-card prose-code:px-1 prose-code:py-0.5 ' +
  'prose-code:rounded-sm prose-code:before:content-none prose-code:after:content-none prose-pre:bg-app-card'

/** Tailwind Typography per-element color modifiers. When `tinted` (a `color` column is
 *  present), headings/body/marker colors become `text-inherit` so the prose div's inline
 *  `color` style applies; links and inline code keep their fixed accent colors. */
function proseClasses(tinted: boolean): string {
  return tinted ? TINTED_PROSE_CLASSES : UNTINTED_PROSE_CLASSES
}

// =============================================================================
// Fit to cell
// =============================================================================

export const MIN_FIT_FONT_PX = 12
export const MAX_FIT_FONT_PX = 320

/**
 * Binary search over integer px in `[min, max]` for the largest size for which `fits`
 * returns true. `fits` is assumed monotonic (larger sizes fit less often). Returns `min`
 * when nothing fits (caller's root then scrolls) and `max` when everything fits.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function fitFontSize(fits: (px: number) => boolean, min = MIN_FIT_FONT_PX, max = MAX_FIT_FONT_PX): number {
  if (!fits(min)) return min
  if (fits(max)) return max
  let lo = min
  let hi = max
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1
    if (fits(mid)) lo = mid
    else hi = mid
  }
  return lo
}

/**
 * Re-fits `proseRef`'s font size to `rootRef`'s content box whenever it resizes or
 * `deps` changes, writing the chosen px straight to the DOM (not React state, which
 * would trip `react-hooks/set-state-in-effect`) — mirrors the mockup's `fit()`.
 * The `ResizeObserver` is only constructed while `enabled`; jsdom has no
 * `ResizeObserver` and non-fit tests never turn `enabled` on.
 */
function useFitFontSize(
  rootRef: React.RefObject<HTMLDivElement | null>,
  proseRef: React.RefObject<HTMLDivElement | null>,
  enabled: boolean,
  deps: React.DependencyList,
): void {
  const [size, setSize] = useState<{ width: number; height: number } | null>(null)

  useEffect(() => {
    // Only the enabled/disabled transition matters here: the useLayoutEffect below
    // ignores `size` entirely while `!enabled`, so a stale value from a previous
    // fit doesn't need clearing.
    if (!enabled) return undefined
    const el = rootRef.current
    if (!el) return undefined
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0]
      if (!entry) return
      setSize({ width: entry.contentRect.width, height: entry.contentRect.height })
    })
    observer.observe(el)
    return () => observer.disconnect()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled])

  useLayoutEffect(() => {
    const prose = proseRef.current
    if (!prose) return
    if (!enabled || !size) {
      prose.style.fontSize = ''
      return
    }
    const fits = (px: number) => {
      prose.style.fontSize = `${px}px`
      return prose.scrollWidth <= size.width && prose.scrollHeight <= size.height
    }
    prose.style.fontSize = `${fitFontSize(fits)}px`
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, size, ...deps])
}

// =============================================================================
// Renderer Component
// =============================================================================

function sameEvaluation(a: { text: string; warnings: string[] }, b: { text: string; warnings: string[] }): boolean {
  return a.text === b.text && a.warnings.length === b.warnings.length && a.warnings.every((w, i) => w === b.warnings[i])
}

export function MarkdownCell({ content, data, status, options, variables, timeRange, cellResults, cellSelections }: CellRendererProps) {
  const table = data[0]
  const row = table ? rowValues(table, 0) : undefined
  const columnTypes = table ? columnTypeMap(table) : undefined

  // Defer evaluation until the cell has executed in sequence — otherwise macros
  // like $variable, $cell.col, or $begin/$end would resolve against uninitialized
  // upstream state and flash stale data or broken links on first paint.
  const evaluated = status === 'success'
    ? evaluateTemplate(content ?? '', { variables, timeRange, cellResults, cellSelections, row, columnTypes, bareColumnsFromRow: true })
    : null

  // Keep the last successful evaluation so a re-run in progress (status idle/loading,
  // with this cell's own last-successful `data` still attached) keeps showing it instead
  // of re-evaluating against `cellResults` that `executeFromCell` has already stripped
  // down for the cells being re-executed (see NotebookRenderer.getAvailableCellResults).
  const [cached, setCached] = useState<{ text: string; warnings: string[] }>({ text: '', warnings: [] })
  if (evaluated && !sameEvaluation(evaluated, cached)) {
    setCached(evaluated)
  }
  const showCached = !evaluated && (status === 'loading' || status === 'idle') && data.length > 0
  const { text: markdownContent, warnings: templateWarnings } = evaluated ?? (showCached ? cached : { text: '', warnings: [] })

  const colors = table && row ? resolveMarkdownColors(table, row) : { warnings: [] as string[] }
  const warnings = [...templateWarnings, ...colors.warnings]

  const fit = !!(options as { fit?: boolean } | undefined)?.fit
  const rootRef = useRef<HTMLDivElement>(null)
  const proseRef = useRef<HTMLDivElement>(null)
  useFitFontSize(rootRef, proseRef, fit, [markdownContent, colors.color, colors.backgroundColor])

  const rootClass = ['flex-1', colors.backgroundColor ? 'rounded-sm' : '', fit ? 'min-h-0 overflow-auto flex text-center' : '']
    .filter(Boolean)
    .join(' ')
  const proseClass = [proseClasses(!!colors.color), fit ? 'm-auto' : ''].filter(Boolean).join(' ')

  return (
    <div
      ref={rootRef}
      className={rootClass}
      style={colors.backgroundColor ? { backgroundColor: colors.backgroundColor } : undefined}
    >
      <div ref={proseRef} className={proseClass} style={colors.color ? { color: colors.color } : undefined}>
        <TemplateWarningBanner warnings={warnings} />
        <Markdown remarkPlugins={[remarkGfm]}>{markdownContent}</Markdown>
      </div>
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function MarkdownCellEditor({ config, onChange, variables, timeRange, cellResults, cellSelections, availableColumns, onRun }: CellEditorProps) {
  const mdConfig = config as MarkdownCellConfig
  const sql = mdConfig.sql ?? DEFAULT_SQL.markdown
  const fit = !!mdConfig.options?.fit

  const sqlValidationErrors = validateMacros(sql, variables, cellResults, cellSelections).errors
  const contentValidationErrors = validateTemplateMacros(
    mdConfig.content ?? '',
    availableColumns ?? [],
    variables,
    cellResults,
    cellSelections,
  ).errors
  const validationErrors = [...sqlValidationErrors, ...contentValidationErrors]

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={sql}
          onChange={(newSql) => onChange({ ...mdConfig, sql: newSql })}
          language="sql"
          placeholder={DEFAULT_SQL.markdown}
          minHeight="120px"
          onRunShortcut={onRun}
        />
      </div>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Markdown Content
        </label>
        <SyntaxEditor
          value={mdConfig.content}
          onChange={(newContent) => onChange({ ...mdConfig, content: newContent })}
          language="markdown"
          placeholder="# Heading&#10;&#10;Your markdown here..."
          minHeight="200px"
          onRunShortcut={onRun}
        />
      </div>
      <label className="flex items-center gap-2 text-sm text-theme-text-secondary">
        <input
          type="checkbox"
          checked={fit}
          onChange={(e) => onChange({ ...mdConfig, options: { ...mdConfig.options, fit: e.target.checked } })}
        />
        Fit to cell
      </label>
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
export const markdownMetadata: CellTypeMetadata = {
  renderer: MarkdownCell,
  EditorComponent: MarkdownCellEditor,

  label: 'Markdown',
  icon: <FileText />,
  description: 'Documentation and headline values',
  showTypeBadge: false,
  defaultHeight: 150,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'markdown' as const,
    content: '# Notes\n\nAdd your documentation here.',
    sql: DEFAULT_SQL.markdown,
  }),

  execute: async (config: CellConfig, { variables, timeRange, cellResults, cellSelections, runQuery }: CellExecutionContext) => {
    const md = config as MarkdownCellConfig
    const sql = substituteMacros(effectiveMarkdownSql(md), variables, timeRange, cellResults, cellSelections)
    const table = await runQuery(sql)
    if (table.numRows === 0) throw new Error('Query returned no rows')
    return { data: [table] }
  },

  getRendererProps: (config: CellConfig, state: CellState) => {
    const md = config as MarkdownCellConfig
    return {
      content: md.content,
      data: state.data,
      status: state.status,
      options: md.options,
    }
  },
}
