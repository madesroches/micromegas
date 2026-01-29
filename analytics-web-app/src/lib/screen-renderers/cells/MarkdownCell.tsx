import { useMemo } from 'react'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import type { CellTypeMetadata, CellRendererProps, CellEditorProps } from '../cell-registry'
import type { MarkdownCellConfig, CellConfig, CellState } from '../notebook-types'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { substituteMacros, validateMacros } from '../notebook-utils'

// =============================================================================
// Renderer Component
// =============================================================================

export function MarkdownCell({ content, variables, timeRange }: CellRendererProps) {
  // Apply macro substitution to markdown content
  const markdownContent = useMemo(() => {
    if (!content) return ''
    return substituteMacros(content, variables, timeRange)
  }, [content, variables, timeRange])

  return (
    <div className="prose prose-invert max-w-none prose-headings:text-theme-text-primary prose-p:text-theme-text-secondary prose-a:text-accent-link prose-strong:text-theme-text-primary prose-code:text-accent-highlight prose-code:bg-app-card prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-code:before:content-none prose-code:after:content-none prose-pre:bg-app-card prose-li:text-theme-text-secondary">
      <Markdown remarkPlugins={[remarkGfm]}>{markdownContent}</Markdown>
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function MarkdownCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const mdConfig = config as MarkdownCellConfig

  // Validate macro references in content
  const validationErrors = useMemo(() => {
    if (!mdConfig.content) return []
    const result = validateMacros(mdConfig.content, variables)
    return result.errors
  }, [mdConfig.content, variables])

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Markdown Content
        </label>
        <SyntaxEditor
          value={mdConfig.content}
          onChange={(content) => onChange({ ...mdConfig, content })}
          language="markdown"
          placeholder="# Heading&#10;&#10;Your markdown here..."
          minHeight="200px"
        />
      </div>
      {validationErrors.length > 0 && (
        <div className="text-red-400 text-sm space-y-1">
          {validationErrors.map((err, i) => (
            <div key={i}>⚠ {err}</div>
          ))}
        </div>
      )}
      <AvailableVariablesPanel variables={variables} timeRange={timeRange} />
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
