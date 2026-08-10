// Ingestion API key management — calls `analytics-web-srv`'s own
// `/api/ingestion-api-keys` routes, which write directly to the
// `ingestion_api_keys` Postgres table. No proxy, no forwarding to ingestion,
// no service credential: ingestion itself exposes no key-management HTTP
// surface at all, so `analytics-web-srv` is the sole admin surface for both
// key tables (#1458). Thin wrapper around the shared factory in
// `api-keys-shared.ts` — see that file for the common
// list/mint/revoke/error-handling logic.
//
// No `importKey` here: the import route exists for the `micromegas-import-keys`
// CLI tool only, which also calls `analytics-web-srv` (with the operator's own
// bearer token) rather than showing a browser form for pasting a legacy key
// in.

import {
  createApiKeysApi,
  ApiKeyListEntry,
  MintApiKeyResponse,
} from './api-keys-shared'

// The server's own hard cap (`MAX_LIMIT` in
// `rust/analytics-web-srv/src/ingestion_keys.rs`), used here as the page size
// for offset-based paging. Requested explicitly on every list call — omitting
// `limit` falls back to the server's lower `DEFAULT_LIMIT` (100), which
// silently truncates the list on any deployment with more than 100 *lifetime*
// keys (revoked keys are never deleted, and `include_revoked` defaults to
// true) with zero indication anything is missing. Exported so the page can
// detect "there may be another page" by comparing the returned row count
// against this same value.
export const MAX_INGESTION_API_KEYS_LIST_LIMIT = 500

export type IngestionApiKeyListEntry = ApiKeyListEntry

export type MintIngestionApiKeyResponse = MintApiKeyResponse

export interface RevokeIngestionApiKeyResponse {
  revoked_at: string
  /** Revocation is not instantaneous — see ingestion's cache TTL. */
  effective_within_seconds: number
}

export interface IngestionApiKeyErrorResponse {
  code?: string
  message: string
}

const api = createApiKeysApi<IngestionApiKeyErrorResponse, RevokeIngestionApiKeyResponse>({
  basePath: '/ingestion-api-keys',
  errorName: 'IngestionApiKeyError',
  maxListLimit: MAX_INGESTION_API_KEYS_LIST_LIMIT,
})

export const IngestionApiKeyError = api.ErrorClass

export const listIngestionApiKeys = api.list
export const mintIngestionApiKey = api.mint
export const revokeIngestionApiKey = api.revoke
