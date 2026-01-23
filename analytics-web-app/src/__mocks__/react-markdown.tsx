import { ReactNode } from 'react'

interface MarkdownProps {
  children?: string
  remarkPlugins?: unknown[]
}

/**
 * Mock react-markdown component for testing.
 * Renders content as HTML using dangerouslySetInnerHTML to parse basic markdown.
 */
function Markdown({ children }: MarkdownProps): ReactNode {
  if (!children) return null

  // Basic markdown to HTML conversion for testing
  let html = children
    // Headers
    .replace(/^### (.+)$/gm, '<h3>$1</h3>')
    .replace(/^## (.+)$/gm, '<h2>$1</h2>')
    .replace(/^# (.+)$/gm, '<h1>$1</h1>')
    // Bold
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    // Italic
    .replace(/\*(.+?)\*/g, '<em>$1</em>')
    // Inline code
    .replace(/`(.+?)`/g, '<code>$1</code>')
    // Links
    .replace(/\[(.+?)\]\((.+?)\)/g, '<a href="$2">$1</a>')
    // Blockquotes
    .replace(/^> (.+)$/gm, '<blockquote>$1</blockquote>')
    // Task lists
    .replace(/^- \[x\] (.+)$/gm, '<li><input type="checkbox" checked disabled />$1</li>')
    .replace(/^- \[ \] (.+)$/gm, '<li><input type="checkbox" disabled />$1</li>')
    // Unordered lists
    .replace(/^- (.+)$/gm, '<li>$1</li>')
    // Ordered lists
    .replace(/^\d+\. (.+)$/gm, '<li>$1</li>')
    // Code blocks
    .replace(/```[\s\S]*?\n([\s\S]*?)```/g, '<pre><code>$1</code></pre>')
    // Tables (basic)
    .replace(/^\|(.+)\|$/gm, (match, content) => {
      if (content.match(/^[\s\-|]+$/)) return '' // Skip separator rows
      const cells = content.split('|').map((c: string) => c.trim())
      return '<tr>' + cells.map((c: string) => `<td>${c}</td>`).join('') + '</tr>'
    })
    // Paragraphs (lines not already converted)
    .replace(/^(?!<[a-z])(.+)$/gm, '<p>$1</p>')

  // Wrap lists
  html = html.replace(/(<li>[\s\S]*?<\/li>\n?)+/g, '<ul>$&</ul>')
  // Wrap tables
  html = html.replace(/(<tr>[\s\S]*?<\/tr>\n?)+/g, '<table>$&</table>')
  // Clean up empty paragraphs
  html = html.replace(/<p><\/p>/g, '')

  return <div dangerouslySetInnerHTML={{ __html: html }} />
}

export default Markdown
