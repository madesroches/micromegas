import { useRef, useEffect, useCallback } from 'react'
import { Braces, Keyboard } from 'lucide-react'
import { formatSQL } from '@/lib/sqlFormatter'

export type SyntaxLanguage = 'sql' | 'markdown'

interface SyntaxEditorProps {
  value: string
  onChange: (value: string) => void
  language: SyntaxLanguage
  placeholder?: string
  className?: string
  minHeight?: string
  onRunShortcut?: () => void
}

// SQL syntax highlighting - returns HTML string
function highlightSql(code: string): string {
  // First escape HTML entities
  let result = code.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')

  // Use placeholder approach to prevent strings and comments from interfering
  // Extract strings first, replace with placeholders
  const strings: string[] = []
  result = result.replace(/'[^']*'/g, (match) => {
    strings.push(match)
    return `__STRING_${strings.length - 1}__`
  })

  // Now highlight comments (safe - strings are placeholders)
  result = result.replace(
    /--[^\n]*/g,
    '<span style="color: var(--syntax-comment)">$&</span>'
  )

  // Highlight SQL keywords (before restoring strings so keywords inside strings aren't highlighted)
  result = result.replace(
    /\b(SELECT|FROM|WHERE|AND|OR|ORDER BY|GROUP BY|LIMIT|OFFSET|AS|ON|JOIN|LEFT|RIGHT|INNER|OUTER|CROSS|FULL|DESC|ASC|DISTINCT|COUNT|SUM|AVG|MIN|MAX|CASE|WHEN|THEN|ELSE|END|IN|NOT|NULL|IS|LIKE|BETWEEN|UNION|ALL|EXISTS|HAVING|WITH|OVER|PARTITION|BY|ROW_NUMBER|RANK|DENSE_RANK|LAG|LEAD|FIRST_VALUE|LAST_VALUE|COALESCE|CAST|EXTRACT|DATE|TIME|TIMESTAMP|INTERVAL|TRUE|FALSE|CREATE|INSERT|UPDATE|DELETE|DROP|ALTER|TABLE|INDEX|VIEW|INTO|VALUES)\b/gi,
    '<span style="color: var(--syntax-keyword)">$&</span>'
  )

  // Highlight variables ($variable_name)
  result = result.replace(
    /\$[a-z_][a-z0-9_]*/gi,
    '<span style="color: var(--syntax-variable)">$&</span>'
  )

  // Highlight numbers (before restoring strings so numbers inside strings aren't highlighted)
  result = result.replace(
    /\b(\d+\.?\d*)\b/g,
    '<span style="color: var(--syntax-number)">$&</span>'
  )

  // Restore strings with highlighting (after other highlighting to preserve string content)
  result = result.replace(/__STRING_(\d+)__/g, (_, i) =>
    `<span style="color: var(--syntax-string)">${strings[parseInt(i)]}</span>`
  )

  // Add trailing newline to match textarea behavior
  return result + '\n'
}

// Markdown syntax highlighting - returns HTML string
function highlightMarkdown(code: string): string {
  // First escape HTML entities
  let result = code.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')

  // Process line by line for headers and blockquotes
  const lines = result.split('\n')
  result = lines
    .map((line) => {
      // Headers (# ## ### etc.)
      if (/^#{1,6}\s/.test(line)) {
        return `<span style="color: var(--syntax-header)">${line}</span>`
      }
      // Blockquotes (> )
      if (/^>\s/.test(line)) {
        return `<span style="color: var(--syntax-blockquote)">${line}</span>`
      }
      // Unordered list items (- or * )
      if (/^[\s]*[-*]\s/.test(line)) {
        return line.replace(
          /^([\s]*)([-*])(\s)/,
          '$1<span style="color: var(--syntax-list)">$2</span>$3'
        )
      }
      // Ordered list items (1. 2. etc.)
      if (/^[\s]*\d+\.\s/.test(line)) {
        return line.replace(
          /^([\s]*)(\d+\.)(\s)/,
          '$1<span style="color: var(--syntax-list)">$2</span>$3'
        )
      }
      return line
    })
    .join('\n')

  // Inline code (`code`)
  result = result.replace(
    /`([^`\n]+)`/g,
    '<span style="color: var(--syntax-code)">$&</span>'
  )

  // Bold (**text** or __text__)
  result = result.replace(
    /(\*\*|__)([^*_\n]+)\1/g,
    '<span style="color: var(--syntax-bold)">$&</span>'
  )

  // Italic (*text* or _text_) - be careful not to match ** or __
  result = result.replace(
    /(?<!\*)\*(?!\*)([^*\n]+)(?<!\*)\*(?!\*)|(?<!_)_(?!_)([^_\n]+)(?<!_)_(?!_)/g,
    '<span style="color: var(--syntax-italic)">$&</span>'
  )

  // Links [text](url)
  result = result.replace(
    /\[([^\]]+)\]\(([^)]+)\)/g,
    '<span style="color: var(--syntax-link)">$&</span>'
  )

  // Add trailing newline to match textarea behavior
  return result + '\n'
}

export function SyntaxEditor({
  value,
  onChange,
  language,
  placeholder,
  className = '',
  minHeight = '150px',
  onRunShortcut,
}: SyntaxEditorProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const preRef = useRef<HTMLPreElement>(null)
  const isSql = language === 'sql'

  // Synchronize scroll between textarea and pre
  const handleScroll = useCallback(() => {
    if (textareaRef.current && preRef.current) {
      preRef.current.scrollTop = textareaRef.current.scrollTop
      preRef.current.scrollLeft = textareaRef.current.scrollLeft
    }
  }, [])

  // Attach scroll listener
  useEffect(() => {
    const textarea = textareaRef.current
    if (textarea) {
      textarea.addEventListener('scroll', handleScroll)
      return () => textarea.removeEventListener('scroll', handleScroll)
    }
  }, [handleScroll])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'Enter' && onRunShortcut) {
        e.preventDefault()
        onRunShortcut()
      }
    },
    [onRunShortcut]
  )

  const handleFormat = useCallback(() => {
    onChange(formatSQL(value))
  }, [value, onChange])

  const highlightedCode = isSql ? highlightSql(value) : highlightMarkdown(value)

  const preClass = isSql
    ? 'absolute inset-0 p-3 font-mono text-xs leading-relaxed whitespace-pre pointer-events-none overflow-hidden m-0'
    : 'absolute inset-0 p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-words pointer-events-none overflow-hidden m-0'

  return (
    <div
      className={`flex flex-col border border-theme-border rounded-md focus-within:border-accent-link bg-app-bg overflow-hidden resize-y ${className}`}
      style={{ minHeight }}
    >
      {/* Editor area */}
      <div className="relative flex-1 min-h-0 overflow-hidden">
        {/* Highlighted code layer (behind) */}
        <pre
          ref={preRef}
          className={preClass}
          aria-hidden="true"
          dangerouslySetInnerHTML={{ __html: highlightedCode }}
        />
        {/* Transparent textarea (in front, captures input) */}
        <textarea
          ref={textareaRef}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={handleKeyDown}
          className="absolute inset-0 w-full h-full p-3 bg-transparent text-transparent caret-theme-text-primary font-mono text-xs leading-relaxed resize-none focus:outline-none"
          style={isSql ? { whiteSpace: 'pre' } : undefined}
          placeholder={placeholder}
          spellCheck={false}
        />
      </div>
      {/* SQL toolbar */}
      {isSql && (
        <div className="flex items-center gap-2 px-3 py-1.5 border-t border-theme-border bg-app-card shrink-0">
          <button
            type="button"
            onClick={handleFormat}
            className="flex items-center gap-1 px-2 py-0.5 text-[11px] text-theme-text-secondary border border-theme-border rounded hover:text-theme-text-primary hover:border-accent-link hover:bg-accent-link/10 transition-all"
            title="Format SQL"
          >
            <Braces className="w-3.5 h-3.5" />
            Format
          </button>
          {onRunShortcut && (
            <span className="flex items-center gap-1 ml-auto text-[11px] text-theme-text-muted">
              <Keyboard className="w-3.5 h-3.5" />
              <kbd className="px-1 py-px text-[10px] bg-app-panel border border-theme-border rounded text-theme-text-secondary">
                Ctrl
              </kbd>
              +
              <kbd className="px-1 py-px text-[10px] bg-app-panel border border-theme-border rounded text-theme-text-secondary">
                Enter
              </kbd>
              {' '}to run
            </span>
          )}
        </div>
      )}
    </div>
  )
}
