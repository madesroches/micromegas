import { useMemo, type AnchorHTMLAttributes } from 'react'
import { X } from 'lucide-react'
import type { Table } from 'apache-arrow'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { AppLink } from '@/components/AppLink'
import { TemplateWarningBanner } from '@/components/TemplateWarningBanner'
import { evaluateTemplate } from '@/lib/screen-renderers/notebook-utils'
import type { VariableValue } from '@/lib/screen-renderers/notebook-types'
import type { Row } from './overlay'

interface EventDetailPanelProps {
  row: Row
  template: string
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
  onClose: () => void
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

export function EventDetailPanel({
  row,
  template,
  variables,
  timeRange,
  cellResults,
  cellSelections,
  onClose,
}: EventDetailPanelProps) {
  const { text: rendered, warnings } = useMemo(() => {
    // Columns win name collisions against variables: `$x` in a Map template
    // means the selected row's `x` column.
    const mergedVars: Record<string, VariableValue> = { ...variables, ...row }
    return evaluateTemplate(template, {
      variables: mergedVars,
      timeRange,
      cellResults,
      cellSelections,
    })
  }, [template, row, variables, timeRange, cellResults, cellSelections])

  return (
    <div className="absolute bottom-4 left-4 w-fit max-w-[50%] max-h-[60%] overflow-auto bg-app-panel border border-theme-border rounded-lg shadow-lg z-10">
      <button
        onClick={onClose}
        className="absolute top-2 right-2 p-1 rounded hover:bg-theme-border transition-colors z-10"
        title="Close"
      >
        <X className="w-4 h-4 text-theme-text-muted" />
      </button>
      <div className="prose prose-invert prose-sm max-w-none pl-4 pr-10 py-3 prose-headings:text-theme-text-primary prose-headings:mt-0 prose-p:text-theme-text-secondary prose-a:text-accent-link prose-strong:text-theme-text-primary prose-code:text-accent-highlight prose-code:bg-app-card prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-code:before:content-none prose-code:after:content-none prose-pre:bg-app-card prose-li:text-theme-text-secondary prose-hr:border-theme-border prose-hr:my-3">
        <TemplateWarningBanner warnings={warnings} />
        <Markdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
          {rendered}
        </Markdown>
      </div>
    </div>
  )
}
