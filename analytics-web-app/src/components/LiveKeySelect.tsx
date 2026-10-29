import { useEffect, useState } from 'react'
import {
  listAllLiveAnalyticsApiKeys,
  type AnalyticsApiKeyListEntry,
} from '@/lib/analytics-api-keys-api'

interface LiveKeySelectProps {
  value: string
  onChange: (keyId: string) => void
}

/** `<select>` of live analytics API keys (name plus short id). Admin-only: the list call is. */
export function LiveKeySelect({ value, onChange }: LiveKeySelectProps) {
  const [keys, setKeys] = useState<AnalyticsApiKeyListEntry[]>([])
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    listAllLiveAnalyticsApiKeys()
      .then((rows) => {
        if (!cancelled) setKeys(rows)
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Failed to load API keys')
      })
    return () => {
      cancelled = true
    }
  }, [])

  return (
    <div>
      <select
        className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm font-mono text-theme-text-primary outline-hidden focus:border-accent-link"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      >
        <option value="">Select an API key…</option>
        {keys.map((k) => (
          <option key={k.key_id} value={k.key_id}>
            {k.name} ({k.key_id.slice(0, 8)})
          </option>
        ))}
      </select>
      {error && <p className="mt-1 text-xs text-accent-error">{error}</p>}
    </div>
  )
}
