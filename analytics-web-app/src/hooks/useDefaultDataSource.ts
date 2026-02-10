import { useState, useEffect } from 'react'
import { listDataSources } from '@/lib/data-sources-api'

/**
 * Hook that returns the name of the default data source.
 * Fetches the list once on mount and finds the default.
 */
export function useDefaultDataSource(): string {
  const [defaultName, setDefaultName] = useState('')

  useEffect(() => {
    let cancelled = false
    listDataSources()
      .then((sources) => {
        if (cancelled) return
        const def = sources.find((s) => s.is_default)
        if (def) {
          setDefaultName(def.name)
        }
      })
      .catch(() => {
        // Silently ignore — empty string means no data source
      })
    return () => {
      cancelled = true
    }
  }, [])

  return defaultName
}
