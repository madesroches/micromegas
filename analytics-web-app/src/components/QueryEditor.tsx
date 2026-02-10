import { useState, useCallback } from 'react'
import { ChevronLeft, ChevronRight, Play } from 'lucide-react'
import { DocumentationLink } from './DocumentationLink'
import { SyntaxEditor } from './SyntaxEditor'

interface QueryEditorProps {
  defaultSql: string
  variables?: { name: string; description: string }[]
  currentValues?: Record<string, string>
  timeRangeLabel?: string
  onRun: (sql: string) => void
  onReset: () => void
  onChange?: (sql: string) => void
  isLoading?: boolean
  error?: string | null
  docLink?: { url: string; label: string }
  /** Content rendered at top of scrollable area, before the SQL editor (only when expanded) */
  topContent?: React.ReactNode
  /** Footer content rendered at bottom of panel (only when expanded) */
  footer?: React.ReactNode
}

export function QueryEditor({
  defaultSql,
  variables = [],
  currentValues = {},
  timeRangeLabel,
  onRun,
  onReset,
  onChange,
  isLoading = false,
  error,
  docLink,
  topContent,
  footer,
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
        {/* Top content (e.g., data source selector) */}
        {topContent}

        {/* SQL Editor with syntax highlighting */}
        <SyntaxEditor
          value={sql}
          onChange={(value) => {
            setSql(value)
            onChange?.(value)
          }}
          language="sql"
          minHeight="192px"
        />

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
        {docLink && <DocumentationLink url={docLink.url} label={docLink.label} />}
      </div>

      {/* Footer */}
      {footer}
    </div>
  )
}
