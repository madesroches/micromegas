import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import type { CellTypeMetadata, CellRendererProps, CellEditorProps } from '../cell-registry'
import type { MarkdownCellConfig, CellConfig, CellState } from '../notebook-types'

// =============================================================================
// Renderer Component
// =============================================================================

export function MarkdownCell({ content }: CellRendererProps) {
  const markdownContent = content || ''

  return (
    <div className="prose prose-invert max-w-none prose-headings:text-theme-text-primary prose-p:text-theme-text-secondary prose-a:text-accent-link prose-strong:text-theme-text-primary prose-code:text-accent-highlight prose-code:bg-app-card prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-code:before:content-none prose-code:after:content-none prose-pre:bg-app-card prose-li:text-theme-text-secondary">
      <Markdown remarkPlugins={[remarkGfm]}>{markdownContent}</Markdown>
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function MarkdownCellEditor({ config, onChange }: CellEditorProps) {
  const mdConfig = config as MarkdownCellConfig

  return (
    <div>
      <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
        Markdown Content
      </label>
      <textarea
        value={mdConfig.content}
        onChange={(e) => onChange({ ...mdConfig, content: e.target.value })}
        className="w-full min-h-[200px] px-3 py-2 bg-app-bg border border-theme-border rounded-md text-theme-text-primary text-sm font-mono focus:outline-none focus:border-accent-link resize-y"
        placeholder="# Heading&#10;&#10;Your markdown here..."
      />
    </div>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

export const markdownMetadata: CellTypeMetadata = {
  renderer: MarkdownCell,
  EditorComponent: MarkdownCellEditor,

  label: 'Markdown',
  icon: 'M',
  description: 'Documentation and notes',
  showTypeBadge: false,
  defaultHeight: 150,

  canBlockDownstream: false,

  createDefaultConfig: () => ({
    type: 'markdown' as const,
    content: '# Notes\n\nAdd your documentation here.',
  }),

  // No execute method - markdown cells don't execute

  getRendererProps: (config: CellConfig, _state: CellState) => ({
    content: (config as MarkdownCellConfig).content,
  }),
}
