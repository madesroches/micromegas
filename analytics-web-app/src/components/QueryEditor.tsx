'use client'

import { useState, useCallback } from 'react'
import { ChevronLeft, ChevronRight, Play } from 'lucide-react'

interface QueryEditorProps {
  defaultSql: string
  variables?: { name: string; description: string }[]
  currentValues?: Record<string, string>
  timeRangeLabel?: string
  onRun: (sql: string) => void
  onReset: () => void
  isLoading?: boolean
  error?: string | null
  docLink?: { url: string; label: string }
}

export function QueryEditor({
  defaultSql,
  variables = [],
  currentValues = {},
  timeRangeLabel,
  onRun,
  onReset,
  isLoading = false,
  error,
  docLink,
}: QueryEditorProps) {
  const [isCollapsed, setIsCollapsed] = useState(true)
  const [sql, setSql] = useState(defaultSql)

  const handleRun = useCallback(() => {
    onRun(sql)
  }, [sql, onRun])

  const handleReset = useCallback(() => {
    setSql(defaultSql)
    onReset()
  }, [defaultSql, onReset])

  // Simple SQL syntax highlighting - returns HTML string
  const highlightSql = (code: string): string => {
    // First escape HTML entities
    let result = code
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')

    // Highlight in order: strings first (so keywords inside strings don't get highlighted)
    result = result.replace(/'[^']*'/g, '<span style="color: var(--accent-success)">$&</span>')
    // Then keywords
    result = result.replace(
      /\b(SELECT|FROM|WHERE|AND|OR|ORDER BY|GROUP BY|LIMIT|OFFSET|AS|ON|JOIN|LEFT|RIGHT|INNER|OUTER|DESC|ASC|DISTINCT|COUNT|SUM|AVG|MIN|MAX|CASE|WHEN|THEN|ELSE|END|IN|NOT|NULL|IS|LIKE|BETWEEN)\b/gi,
      '<span style="color: var(--accent-highlight)">$&</span>'
    )
    // Then variables
    result = result.replace(/\$[a-z_][a-z0-9_]*/gi, '<span style="color: var(--accent-variable)">$&</span>')

    // Add a trailing newline to match textarea behavior (prevents content jump)
    return result + '\n'
  }

  if (isCollapsed) {
    return (
      <div className="hidden md:flex w-12 bg-app-panel border-l border-theme-border flex-col">
        <div className="p-2">
          <button
            onClick={() => setIsCollapsed(false)}
            className="w-8 h-8 flex items-center justify-center text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-border rounded transition-colors"
            title="Expand SQL Panel"
          >
            <ChevronLeft className="w-4 h-4" />
          </button>
        </div>
      </div>
    )
  }

  return (
    <div className="hidden md:flex w-80 lg:w-96 bg-app-panel border-l border-theme-border flex-col">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 bg-app-card border-b border-theme-border">
        <div className="flex items-center gap-2">
          <button
            onClick={() => setIsCollapsed(true)}
            className="w-6 h-6 flex items-center justify-center text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-border rounded transition-colors"
            title="Collapse panel"
          >
            <ChevronRight className="w-4 h-4" />
          </button>
          <span className="text-sm font-semibold text-theme-text-primary">SQL Query</span>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={handleReset}
            className="px-2.5 py-1 text-xs text-theme-text-secondary border border-theme-border rounded hover:bg-theme-border hover:text-theme-text-primary transition-colors"
          >
            Reset
          </button>
          <button
            onClick={handleRun}
            disabled={isLoading}
            className="flex items-center gap-1 px-2.5 py-1 text-xs bg-accent-success text-white rounded hover:opacity-90 disabled:bg-theme-border disabled:cursor-not-allowed transition-colors"
          >
            <Play className="w-3 h-3" />
            Run
          </button>
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-auto p-4">
        {/* SQL Editor with syntax highlighting overlay */}
        <div className="relative h-48 border border-theme-border rounded-md focus-within:border-accent-link bg-app-bg overflow-hidden">
          {/* Highlighted code layer (behind) */}
          <pre
            className="absolute inset-0 p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-words pointer-events-none overflow-hidden m-0"
            aria-hidden="true"
            dangerouslySetInnerHTML={{ __html: highlightSql(sql) }}
          />
          {/* Transparent textarea (in front, captures input) */}
          <textarea
            value={sql}
            onChange={(e) => setSql(e.target.value)}
            className="absolute inset-0 w-full h-full p-3 bg-transparent text-transparent caret-theme-text-primary font-mono text-xs leading-relaxed resize-none focus:outline-none"
            spellCheck={false}
          />
        </div>

        {/* Error */}
        {error && (
          <div className="mt-3 p-3 bg-accent-error/10 border border-accent-error/50 rounded-md">
            <p className="text-xs text-accent-error">{error}</p>
          </div>
        )}

        {/* Variables */}
        {variables.length > 0 && (
          <div className="mt-4">
            <h4 className="text-xs font-semibold uppercase tracking-wide text-theme-text-muted mb-2">
              Variables
            </h4>
            <div className="text-xs text-theme-text-muted space-y-1">
              {variables.map((v) => (
                <div key={v.name}>
                  <code className="px-1.5 py-0.5 bg-theme-border rounded text-accent-variable">
                    ${v.name}
                  </code>{' '}
                  - {v.description}
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Current Values */}
        {Object.keys(currentValues).length > 0 && (
          <div className="mt-4">
            <h4 className="text-xs font-semibold uppercase tracking-wide text-theme-text-muted mb-2">
              Current Values
            </h4>
            <div className="text-xs text-theme-text-muted space-y-1">
              {Object.entries(currentValues).map(([key, value]) => (
                <div key={key}>
                  <code className="px-1.5 py-0.5 bg-theme-border rounded text-accent-variable">
                    ${key}
                  </code>{' '}
                  = <span className="text-theme-text-secondary">{value}</span>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Time Range */}
        {timeRangeLabel && (
          <div className="mt-4">
            <h4 className="text-xs font-semibold uppercase tracking-wide text-theme-text-muted mb-2">
              Time Range
            </h4>
            <p className="text-xs text-theme-text-muted">
              Applied implicitly via FlightSQL headers.
              <br />
              Current: <span className="text-theme-text-primary">{timeRangeLabel}</span>
            </p>
          </div>
        )}

        {/* Documentation Link */}
        {docLink && (
          <div className="mt-4">
            <h4 className="text-xs font-semibold uppercase tracking-wide text-theme-text-muted mb-2">
              Documentation
            </h4>
            <a
              href={docLink.url}
              target="_blank"
              rel="noopener noreferrer"
              className="text-xs text-accent-link hover:underline"
            >
              {docLink.label}
            </a>
          </div>
        )}
      </div>
    </div>
  )
}
