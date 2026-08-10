// Analytics API key management — calls `analytics-web-srv`'s own
// `/api/analytics-api-keys` routes directly (no proxy needed: this service
// hosts the analytics-key store itself). Modeled on `data-sources-api.ts`'s
// `handleResponse`/error-class shape.
//
// No `importKey` here: the import route exists for the `micromegas-import-keys`
// CLI tool only (see the design doc's §5) — a browser form for pasting a
// legacy key string in would reintroduce the "key transits a browser"
// exposure mint already avoids.

import { authenticatedFetch, getApiBase } from './api'

// The server's own hard cap (`MAX_LIMIT` in `rust/public/src/servers/api_keys.rs`),
// used here as the page size for offset-based paging. Requested explicitly on
// every list call — omitting `limit` falls back to the server's lower
// `DEFAULT_LIMIT` (100), which silently truncates the list on any deployment
// with more than 100 *lifetime* keys (revoked keys are never deleted, and
// `include_revoked` defaults to true) with zero indication anything is
// missing. Exported so the page can detect "there may be another page" by
// comparing the returned row count against this same value.
export const MAX_ANALYTICS_API_KEYS_LIST_LIMIT = 500

export interface AnalyticsApiKeyListEntry {
  key_id: string
  name: string
  created_at: string
  created_by: string
  last_used_at: string | null
  revoked_at: string | null
  revoked_by: string | null
}

export interface MintAnalyticsApiKeyResponse {
  key_id: string
  name: string
  created_at: string
  /** The cleartext key, returned exactly once. Never persisted client-side. */
  key: string
}

export interface RevokeAnalyticsApiKeyResponse {
  revoked_at: string
}

export interface AnalyticsApiKeyErrorResponse {
  code: string
  message: string
}

export class AnalyticsApiKeyError extends Error {
  constructor(
    public code: string,
    message: string,
    public status: number
  ) {
    super(message)
    this.name = 'AnalyticsApiKeyError'
  }
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    let errorData: AnalyticsApiKeyErrorResponse | undefined
    try {
      errorData = await response.json()
    } catch {
      // Ignore JSON parse errors
    }
    throw new AnalyticsApiKeyError(
      errorData?.code ?? 'UNKNOWN_ERROR',
      errorData?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
  return response.json()
}

export async function listAnalyticsApiKeys(
  includeRevoked = true,
  offset = 0
): Promise<AnalyticsApiKeyListEntry[]> {
  const response = await authenticatedFetch(
    `${getApiBase()}/analytics-api-keys?limit=${MAX_ANALYTICS_API_KEYS_LIST_LIMIT}&offset=${offset}&include_revoked=${includeRevoked}`
  )
  return handleResponse<AnalyticsApiKeyListEntry[]>(response)
}

export async function mintAnalyticsApiKey(name: string): Promise<MintAnalyticsApiKeyResponse> {
  const response = await authenticatedFetch(`${getApiBase()}/analytics-api-keys`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  })
  return handleResponse<MintAnalyticsApiKeyResponse>(response)
}

export async function revokeAnalyticsApiKey(
  keyId: string
): Promise<RevokeAnalyticsApiKeyResponse> {
  const response = await authenticatedFetch(
    `${getApiBase()}/analytics-api-keys/${encodeURIComponent(keyId)}`,
    { method: 'DELETE' }
  )
  return handleResponse<RevokeAnalyticsApiKeyResponse>(response)
}
