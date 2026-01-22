import { CellRendererProps, registerCellRenderer } from '../cell-registry'

/**
 * Simple markdown renderer.
 * For now, handles basic markdown: headers, lists, bold, italic, links.
 */
function renderMarkdown(content: string): React.ReactNode {
  const lines = content.split('\n')
  const elements: React.ReactNode[] = []
  let listItems: string[] = []
  let listKey = 0

  const flushList = () => {
    if (listItems.length > 0) {
      elements.push(
        <ul key={`list-${listKey++}`} className="list-disc list-inside mb-4 text-theme-text-secondary">
          {listItems.map((item, i) => (
            <li key={i}>{renderInline(item)}</li>
          ))}
        </ul>
      )
      listItems = []
    }
  }

  const renderInline = (text: string): React.ReactNode => {
    // Handle bold, italic, links, and code
    const parts: React.ReactNode[] = []
    let remaining = text
    let key = 0

    while (remaining.length > 0) {
      // Bold: **text**
      const boldMatch = remaining.match(/^\*\*(.+?)\*\*/)
      if (boldMatch) {
        parts.push(<strong key={key++}>{boldMatch[1]}</strong>)
        remaining = remaining.slice(boldMatch[0].length)
        continue
      }

      // Italic: *text*
      const italicMatch = remaining.match(/^\*(.+?)\*/)
      if (italicMatch) {
        parts.push(<em key={key++}>{italicMatch[1]}</em>)
        remaining = remaining.slice(italicMatch[0].length)
        continue
      }

      // Code: `text`
      const codeMatch = remaining.match(/^`(.+?)`/)
      if (codeMatch) {
        parts.push(
          <code key={key++} className="px-1 py-0.5 bg-app-card rounded text-accent-highlight font-mono text-sm">
            {codeMatch[1]}
          </code>
        )
        remaining = remaining.slice(codeMatch[0].length)
        continue
      }

      // Link: [text](url)
      const linkMatch = remaining.match(/^\[(.+?)\]\((.+?)\)/)
      if (linkMatch) {
        parts.push(
          <a
            key={key++}
            href={linkMatch[2]}
            target="_blank"
            rel="noopener noreferrer"
            className="text-accent-link hover:underline"
          >
            {linkMatch[1]}
          </a>
        )
        remaining = remaining.slice(linkMatch[0].length)
        continue
      }

      // Regular text - take one character
      parts.push(remaining[0])
      remaining = remaining.slice(1)
    }

    return parts.length === 1 ? parts[0] : <>{parts}</>
  }

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]

    // Empty line
    if (line.trim() === '') {
      flushList()
      continue
    }

    // Header: # ## ###
    const headerMatch = line.match(/^(#{1,3})\s+(.+)$/)
    if (headerMatch) {
      flushList()
      const level = headerMatch[1].length
      const text = headerMatch[2]
      if (level === 1) {
        elements.push(
          <h1 key={i} className="text-xl font-bold text-theme-text-primary mb-3">
            {renderInline(text)}
          </h1>
        )
      } else if (level === 2) {
        elements.push(
          <h2 key={i} className="text-lg font-semibold text-theme-text-primary mb-2">
            {renderInline(text)}
          </h2>
        )
      } else {
        elements.push(
          <h3 key={i} className="text-base font-medium text-theme-text-primary mb-2">
            {renderInline(text)}
          </h3>
        )
      }
      continue
    }

    // List item: - or *
    const listMatch = line.match(/^[-*]\s+(.+)$/)
    if (listMatch) {
      listItems.push(listMatch[1])
      continue
    }

    // Regular paragraph
    flushList()
    elements.push(
      <p key={i} className="text-theme-text-secondary mb-3">
        {renderInline(line)}
      </p>
    )
  }

  flushList()
  return elements
}

export function MarkdownCell({ content, isEditing, onContentChange }: CellRendererProps) {
  const markdownContent = content || ''

  // In edit mode, show a textarea
  if (isEditing && onContentChange) {
    return (
      <div className="space-y-2">
        <div className="text-xs text-theme-text-muted">Preview:</div>
        <div className="prose prose-invert max-w-none">{renderMarkdown(markdownContent)}</div>
      </div>
    )
  }

  // In view mode, render the markdown
  return <div className="prose prose-invert max-w-none">{renderMarkdown(markdownContent)}</div>
}

// Register this cell renderer
registerCellRenderer('markdown', MarkdownCell)
