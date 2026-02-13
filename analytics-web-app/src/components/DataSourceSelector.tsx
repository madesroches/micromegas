import { useState, useEffect } from 'react'
import { Database, AlertCircle } from 'lucide-react'
import { getDataSourceList, DataSourceSummary } from '@/lib/data-sources-api'

interface DataSourceSelectorProps {
  value: string
  onChange: (name: string) => void
  datasourceVariables?: string[]
  /** Show 'notebook' as a data source option (for cells inside notebooks) */
  showNotebookOption?: boolean
}

/**
 * Labeled data source selector with heading. Use this in query panels and editors.
 * Wraps DataSourceSelector with a standard h4 label.
 * Returns null when DataSourceSelector is hidden (single source or loading).
 */
export function DataSourceField({ value, onChange, datasourceVariables, showNotebookOption, className = 'mb-4' }: DataSourceSelectorProps & { className?: string }) {
  const [sources, setSources] = useState<DataSourceSummary[]>([])

  useEffect(() => {
    let cancelled = false
    getDataSourceList().then((data) => { if (!cancelled) setSources(data) }).catch(() => {})
    return () => { cancelled = true }
  }, [])

  const hasVariables = datasourceVariables && datasourceVariables.length > 0

  // Hide entirely when selector would return null (<=1 sources, no variables, no notebook option)
  if (sources.length <= 1 && !hasVariables && !showNotebookOption) return null

  return (
    <div className={className}>
      <h4 className="text-xs font-semibold uppercase tracking-wide text-theme-text-muted mb-2">Data Source</h4>
      <DataSourceSelector value={value} onChange={onChange} datasourceVariables={datasourceVariables} showNotebookOption={showNotebookOption} />
    </div>
  )
}

export function DataSourceSelector({ value, onChange, datasourceVariables, showNotebookOption }: DataSourceSelectorProps) {
  const [sources, setSources] = useState<DataSourceSummary[]>([])
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    getDataSourceList()
      .then((data) => {
        if (!cancelled) setSources(data)
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : 'Failed to load data sources')
        }
      })
    return () => {
      cancelled = true
    }
  }, [])

  const hasVariables = datasourceVariables && datasourceVariables.length > 0

  if (error) {
    return (
      <div className="flex items-center gap-1.5 text-xs text-accent-error" title={error}>
        <AlertCircle className="w-3.5 h-3.5" />
        <span>Data sources unavailable</span>
      </div>
    )
  }

  // Don't render if there's only one data source, no variables, and no notebook option
  if (sources.length <= 1 && !hasVariables && !showNotebookOption) return null

  const notebookOption = showNotebookOption ? (
    <option key="notebook" value="notebook">
      Notebook (local)
    </option>
  ) : null

  return (
    <div className="flex items-center gap-1.5">
      <Database className="w-3.5 h-3.5 text-theme-text-muted" />
      <select
        className="bg-app-panel border border-theme-border rounded px-2 py-1 text-xs text-theme-text-primary outline-none focus:border-accent-link cursor-pointer"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      >
        {hasVariables ? (
          <>
            {notebookOption}
            <optgroup label="Variables">
              {datasourceVariables.map((name) => (
                <option key={`$${name}`} value={`$${name}`}>
                  ${name}
                </option>
              ))}
            </optgroup>
            <optgroup label="Data Sources">
              {sources.map((s) => (
                <option key={s.name} value={s.name}>
                  {s.name}
                  {s.is_default ? ' (default)' : ''}
                </option>
              ))}
            </optgroup>
          </>
        ) : (
          <>
            {notebookOption}
            {sources.map((s) => (
              <option key={s.name} value={s.name}>
                {s.name}
                {s.is_default ? ' (default)' : ''}
              </option>
            ))}
          </>
        )}
      </select>
    </div>
  )
}
