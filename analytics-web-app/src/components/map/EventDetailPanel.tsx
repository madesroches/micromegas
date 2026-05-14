import { useMemo, type AnchorHTMLAttributes } from 'react'
import { X } from 'lucide-react'
import type { Table } from 'apache-arrow'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { AppLink } from '@/components/AppLink'
import { substituteMacros } from '@/lib/screen-renderers/notebook-utils'
import type { VariableValue } from '@/lib/screen-renderers/notebook-types'
import type { MapEvent } from './MapViewer'

interface EventDetailPanelProps {
  event: MapEvent
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
  event,
  template,
  variables,
  timeRange,
  cellResults,
  cellSelections,
  onClose,
}: EventDetailPanelProps) {
  const rendered = useMemo(() => {
    // Columns win name collisions against variables: `$x` in a Map template
    // means the selected row's `x` column.
    const mergedVars: Record<string, VariableValue> = { ...variables, ...event.row }
    return substituteMacros(template, mergedVars, timeRange, cellResults, cellSelections)
  }, [template, event, variables, timeRange, cellResults, cellSelections])

  return (
    <div className="absolute bottom-4 left-4 w-80 max-h-[60%] overflow-y-auto bg-app-panel border border-theme-border rounded-lg shadow-lg z-10">
      <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
        <h3 className="text-sm font-semibold text-theme-text-primary">Event Details</h3>
        <button
          onClick={onClose}
          className="p-1 rounded hover:bg-theme-border transition-colors"
          title="Close"
        >
          <X className="w-4 h-4 text-theme-text-muted" />
        </button>
      </div>
      <div className="prose prose-invert prose-sm max-w-none p-4 prose-headings:text-theme-text-primary prose-p:text-theme-text-secondary prose-a:text-accent-link prose-strong:text-theme-text-primary prose-code:text-accent-highlight prose-code:bg-app-card prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-code:before:content-none prose-code:after:content-none prose-pre:bg-app-card prose-li:text-theme-text-secondary">
        <Markdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
          {rendered}
        </Markdown>
      </div>
    </div>
  )
}
