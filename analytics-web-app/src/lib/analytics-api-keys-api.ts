// Analytics API key management — calls `analytics-web-srv`'s own
// `/api/analytics-api-keys` routes directly (no proxy needed: this service
// hosts the analytics-key store itself). Thin wrapper around the shared
// factory in `api-keys-shared.ts` — see that file for the common
// list/mint/revoke/error-handling logic.
//
// No `importKey` here: the import route exists for the `micromegas-import-keys`
// CLI tool only (see the design doc's §5) — a browser form for pasting a
// legacy key string in would reintroduce the "key transits a browser"
// exposure mint already avoids.

import {
  createApiKeysApi,
  ApiKeyListEntry,
  MintApiKeyResponse,
} from './api-keys-shared'

// The server's own hard cap (`MAX_LIMIT` in `rust/analytics-web-srv/src/analytics_keys.rs`),
// used here as the page size for offset-based paging. Requested explicitly on
// every list call — omitting `limit` falls back to the server's lower
// `DEFAULT_LIMIT` (100), which silently truncates the list on any deployment
// with more than 100 *lifetime* keys (revoked keys are never deleted, and
// `include_revoked` defaults to true) with zero indication anything is
// missing. Exported so the page can detect "there may be another page" by
// comparing the returned row count against this same value.
export const MAX_ANALYTICS_API_KEYS_LIST_LIMIT = 500

export type AnalyticsApiKeyListEntry = ApiKeyListEntry

export type MintAnalyticsApiKeyResponse = MintApiKeyResponse

export interface RevokeAnalyticsApiKeyResponse {
  revoked_at: string
}

export interface AnalyticsApiKeyErrorResponse {
  code: string
  message: string
}

const api = createApiKeysApi<AnalyticsApiKeyErrorResponse, RevokeAnalyticsApiKeyResponse>({
  basePath: '/analytics-api-keys',
  errorName: 'AnalyticsApiKeyError',
  maxListLimit: MAX_ANALYTICS_API_KEYS_LIST_LIMIT,
})

export const AnalyticsApiKeyError = api.ErrorClass

export const listAnalyticsApiKeys = api.list
export const mintAnalyticsApiKey = api.mint
export const revokeAnalyticsApiKey = api.revoke
