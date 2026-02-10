import { useState, useEffect } from 'react'
import { Database, AlertCircle } from 'lucide-react'
import { getDataSourceList, DataSourceSummary } from '@/lib/data-sources-api'

interface DataSourceSelectorProps {
  value: string
  onChange: (name: string) => void
  showWithSingleSource?: boolean
}

export function DataSourceSelector({ value, onChange, showWithSingleSource = false }: DataSourceSelectorProps) {
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

  if (error) {
    return (
      <div className="flex items-center gap-1.5 text-xs text-accent-error" title={error}>
        <AlertCircle className="w-3.5 h-3.5" />
        <span>Data sources unavailable</span>
      </div>
    )
  }

  // Don't render if there's only one data source (unless forced to show)
  if (sources.length <= 1 && !showWithSingleSource) return null

  return (
    <div className="flex items-center gap-1.5">
      <Database className="w-3.5 h-3.5 text-theme-text-muted" />
      <select
        className="bg-app-panel border border-theme-border rounded px-2 py-1 text-xs text-theme-text-primary outline-none focus:border-accent-link cursor-pointer"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      >
        {sources.map((s) => (
          <option key={s.name} value={s.name}>
            {s.name}
            {s.is_default ? ' (default)' : ''}
          </option>
        ))}
      </select>
    </div>
  )
}
