import { useState, useEffect } from 'react'
import { getDataSourceList } from '@/lib/data-sources-api'

/**
 * Hook that returns the name of the default data source.
 * Uses a module-level cached fetch shared with all other consumers.
 */
export function useDefaultDataSource(): string {
  const [defaultName, setDefaultName] = useState('')

  useEffect(() => {
    let cancelled = false
    getDataSourceList()
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
