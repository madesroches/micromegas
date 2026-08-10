// Ingestion API key management — calls `analytics-web-srv`'s server-side
// proxy (`/api/ingestion-api-keys`), which forwards to ingestion's own
// `/auth/api_keys` routes under this service's privileged service credential.
// The browser can't call ingestion directly: the `id_token` cookie is
// `http_only`, so there's no bearer token for browser JS to attach. Modeled
// on `data-sources-api.ts`'s `handleResponse`/error-class shape.
//
// No `importKey` here: the import route exists for the `micromegas-import-keys`
// CLI tool only, which calls ingestion directly with the operator's own
// bearer token — it doesn't go through this proxy at all.

import { authenticatedFetch, getApiBase } from './api'

// The server's own hard cap (`MAX_LIMIT` in `rust/public/src/servers/api_keys.rs`),
// used here as the page size for offset-based paging. Requested explicitly on
// every list call — omitting `limit` falls back to the server's lower
// `DEFAULT_LIMIT` (100), which silently truncates the list on any deployment
// with more than 100 *lifetime* keys (revoked keys are never deleted, and
// `include_revoked` defaults to true) with zero indication anything is
// missing. Exported so the page can detect "there may be another page" by
// comparing the returned row count against this same value.
export const MAX_INGESTION_API_KEYS_LIST_LIMIT = 500

export interface IngestionApiKeyListEntry {
  key_id: string
  name: string
  created_at: string
  created_by: string
  last_used_at: string | null
  revoked_at: string | null
  revoked_by: string | null
}

export interface MintIngestionApiKeyResponse {
  key_id: string
  name: string
  created_at: string
  /** The cleartext key, returned exactly once. Never persisted client-side. */
  key: string
}

export interface RevokeIngestionApiKeyResponse {
  revoked_at: string
  /** Revocation is not instantaneous — see ingestion's cache TTL. */
  effective_within_seconds: number
}

export interface IngestionApiKeyErrorResponse {
  code?: string
  message: string
}

export class IngestionApiKeyError extends Error {
  constructor(
    public code: string,
    message: string,
    public status: number
  ) {
    super(message)
    this.name = 'IngestionApiKeyError'
  }
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    let errorData: IngestionApiKeyErrorResponse | undefined
    try {
      errorData = await response.json()
    } catch {
      // Ignore JSON parse errors
    }
    throw new IngestionApiKeyError(
      errorData?.code ?? 'UNKNOWN_ERROR',
      errorData?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
  return response.json()
}

export async function listIngestionApiKeys(
  includeRevoked = true,
  offset = 0
): Promise<IngestionApiKeyListEntry[]> {
  const response = await authenticatedFetch(
    `${getApiBase()}/ingestion-api-keys?limit=${MAX_INGESTION_API_KEYS_LIST_LIMIT}&offset=${offset}&include_revoked=${includeRevoked}`
  )
  return handleResponse<IngestionApiKeyListEntry[]>(response)
}

export async function mintIngestionApiKey(name: string): Promise<MintIngestionApiKeyResponse> {
  const response = await authenticatedFetch(`${getApiBase()}/ingestion-api-keys`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  })
  return handleResponse<MintIngestionApiKeyResponse>(response)
}

export async function revokeIngestionApiKey(
  keyId: string
): Promise<RevokeIngestionApiKeyResponse> {
  const response = await authenticatedFetch(
    `${getApiBase()}/ingestion-api-keys/${encodeURIComponent(keyId)}`,
    { method: 'DELETE' }
  )
  return handleResponse<RevokeIngestionApiKeyResponse>(response)
}
