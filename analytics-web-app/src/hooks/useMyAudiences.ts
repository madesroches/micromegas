import { useCallback, useState } from 'react'
import { AudienceGrantError, fetchMyAudiences, type MyAudiences } from '@/lib/audience-grants-api'

export interface UseMyAudiencesReturn {
  me: MyAudiences | null
  selfServiceOff: boolean
  /** `/visible` and `/my-audiences` share the same `--disable-auth` gate, so they always fail
   *  together -- a caller who also loads `/visible` doesn't need a second `AUTH_DISABLED` check
   *  of its own. */
  authDisabled: boolean
  /** A genuine fetch failure (500, network, parse) -- distinct from `selfServiceOff`, which is a
   *  normal 403. `null` while loading or once loaded successfully. */
  error: string | null
  reload: () => Promise<void>
}

/**
 * `GET {base}/api/audience-grants/my-audiences`, wrapped in the load/error state every
 * self-service-mint consumer needs. Extracted from `AudienceAccessPage.tsx` (#1544) so
 * `IngestionApiKeysPage.tsx`'s non-admin panel can share the same identity/knob-detection logic
 * instead of a second copy.
 */
export function useMyAudiences(): UseMyAudiencesReturn {
  const [me, setMe] = useState<MyAudiences | null>(null)
  const [selfServiceOff, setSelfServiceOff] = useState(false)
  const [authDisabled, setAuthDisabled] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const reload = useCallback(async () => {
    try {
      const result = await fetchMyAudiences()
      setMe(result)
      setSelfServiceOff(false)
      setError(null)
    } catch (err) {
      if (err instanceof AudienceGrantError && err.code === 'AUTH_DISABLED') {
        setAuthDisabled(true)
      } else if (err instanceof AudienceGrantError && err.status === 403) {
        setMe(null)
        setSelfServiceOff(true)
        setError(null)
      } else {
        // Genuine failure (500, network, parse) -- keep `me` at whatever it was (most likely
        // still null) and surface a retryable error instead of silently leaving the Mint
        // affordances gated open on a null identity.
        setError(err instanceof AudienceGrantError ? err.message : 'Failed to load your audiences')
      }
    }
  }, [])

  return { me, selfServiceOff, authDisabled, error, reload }
}
