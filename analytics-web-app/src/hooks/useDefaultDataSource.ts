import { useState, useEffect } from 'react'
import { getDataSourceList } from '@/lib/data-sources-api'

interface DefaultDataSourceState {
  name: string
  error: string | null
}

/**
 * Hook that returns the name of the default data source.
 * Uses a module-level cached fetch shared with all other consumers.
 */
export function useDefaultDataSource(): DefaultDataSourceState {
  const [state, setState] = useState<DefaultDataSourceState>({ name: '', error: null })

  useEffect(() => {
    let cancelled = false
    getDataSourceList()
      .then((sources) => {
        if (cancelled) return
        const def = sources.find((s) => s.is_default)
        if (def) {
          setState({ name: def.name, error: null })
        } else {
          setState({ name: '', error: 'No default data source configured. An admin must add one in Admin > Data Sources.' })
        }
      })
      .catch((err) => {
        if (cancelled) return
        setState({ name: '', error: `Failed to load data sources: ${err instanceof Error ? err.message : 'unknown error'}` })
      })
    return () => {
      cancelled = true
    }
  }, [])

  return state
}
