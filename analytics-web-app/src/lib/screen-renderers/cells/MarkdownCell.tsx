import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { CellRendererProps, registerCellRenderer } from '../cell-registry'

export function MarkdownCell({ content }: CellRendererProps) {
  const markdownContent = content || ''

  return (
    <div className="prose prose-invert max-w-none prose-headings:text-theme-text-primary prose-p:text-theme-text-secondary prose-a:text-accent-link prose-strong:text-theme-text-primary prose-code:text-accent-highlight prose-code:bg-app-card prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-code:before:content-none prose-code:after:content-none prose-pre:bg-app-card prose-li:text-theme-text-secondary">
      <Markdown remarkPlugins={[remarkGfm]}>{markdownContent}</Markdown>
    </div>
  )
}

// Register this cell renderer
registerCellRenderer('markdown', MarkdownCell)
