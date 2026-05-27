import { useMemo, type AnchorHTMLAttributes } from 'react'
import type { DataType, Table } from 'apache-arrow'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { AppLink } from '@/components/AppLink'
import { TemplateWarningBanner } from '@/components/TemplateWarningBanner'
import { evaluateTemplate } from '@/lib/screen-renderers/notebook-utils'
import type { VariableValue } from '@/lib/screen-renderers/notebook-types'

interface EventDetailContentProps {
  row: Record<string, unknown>
  columnTypes: Map<string, DataType>
  template: string
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
  // Tighter, symmetric padding for the transient hover tooltip, which has no
  // close button and so doesn't need the `pr-10` gutter the docked panel keeps.
  compact?: boolean
}

// `node` is the AST node react-markdown passes to component overrides;
// destructure-and-drop it so React doesn't warn about an unknown DOM prop.
function MarkdownLink({
  href,
  children,
  node: _node,
  ...rest
}: AnchorHTMLAttributes<HTMLAnchorElement> & { node?: unknown }) {
  if (!href) {
    return <a {...rest}>{children}</a>
  }
  if (/^https?:/i.test(href) || href.startsWith('//') || href.startsWith('mailto:')) {
    return (
      <a href={href} target="_blank" rel="noopener noreferrer" {...rest}>
        {children}
      </a>
    )
  }
  return (
    <AppLink href={href} className={rest.className} title={rest.title}>
      {children}
    </AppLink>
  )
}

/**
 * Presentational detail-template renderer shared by the docked
 * `EventDetailPanel` and the transient `MapHoverTooltip`. Holds the
 * template-eval + warning banner + Markdown block so both surfaces render
 * byte-identical content with different chrome.
 */
export function EventDetailContent({
  row,
  columnTypes,
  template,
  variables,
  timeRange,
  cellResults,
  cellSelections,
  compact = false,
}: EventDetailContentProps) {
  const { text: rendered, warnings } = useMemo(() => {
    // Bare `$col` macros resolve from the selected row (with their Arrow type)
    // before falling back to a notebook variable: columns win name collisions,
    // timestamps render RFC3339, and `format_value($col, unit)` gets the raw
    // full-precision value.
    return evaluateTemplate(template, {
      variables,
      timeRange,
      cellResults,
      cellSelections,
      row,
      columnTypes,
      bareColumnsFromRow: true,
    })
  }, [template, row, columnTypes, variables, timeRange, cellResults, cellSelections])

  return (
    <div
      className={`prose prose-invert prose-sm max-w-none ${compact ? 'px-3 py-2' : 'pl-4 pr-10 py-3'} prose-headings:text-theme-text-primary prose-headings:mt-0 prose-p:text-theme-text-secondary prose-a:text-accent-link prose-strong:text-theme-text-primary prose-code:text-accent-highlight prose-code:bg-app-card prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-code:before:content-none prose-code:after:content-none prose-pre:bg-app-card prose-li:text-theme-text-secondary prose-hr:border-theme-border prose-hr:my-3`}
    >
      <TemplateWarningBanner warnings={warnings} />
      <Markdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
        {rendered}
      </Markdown>
    </div>
  )
}
