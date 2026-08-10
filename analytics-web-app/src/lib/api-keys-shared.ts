// Shared factory for the analytics/ingestion API-key client modules
// (`analytics-api-keys-api.ts` and `ingestion-api-keys-api.ts`). Both talk to
// a REST resource that's identically shaped (list/mint/revoke, offset-based
// paging, `{code, message}`-ish errors) but live at different base paths and
// have small, deliberate differences in their response/error types — see
// each thin module for exactly what differs and why. Modeled on
// `data-sources-api.ts`'s `handleResponse`/error-class shape.

import { authenticatedFetch, getApiBase } from './api'

export interface ApiKeyListEntry {
  key_id: string
  name: string
  created_at: string
  created_by: string
  last_used_at: string | null
  revoked_at: string | null
  revoked_by: string | null
}

export interface MintApiKeyResponse {
  key_id: string
  name: string
  created_at: string
  /** The cleartext key, returned exactly once. Never persisted client-side. */
  key: string
}

interface ErrorResponseShape {
  code?: string
  message: string
}

/** Constructor shape shared by the per-module error classes (`AnalyticsApiKeyError`, `IngestionApiKeyError`). */
export type ApiKeyErrorConstructor = new (code: string, message: string, status: number) => Error

function createApiKeyErrorClass(name: string): ApiKeyErrorConstructor {
  const ErrorClass = class extends Error {
    code: string
    status: number
    constructor(code: string, message: string, status: number) {
      super(message)
      this.code = code
      this.status = status
      this.name = name
    }
  }
  return ErrorClass
}

export interface ApiKeysApiConfig {
  /** e.g. '/analytics-api-keys' — appended to `getApiBase()`. */
  basePath: string
  /** `.name` of the per-module error class, e.g. 'AnalyticsApiKeyError'. */
  errorName: string
  /** The server's hard cap on `limit`, used as the page size for offset-based paging. */
  maxListLimit: number
}

export interface ApiKeysApi<TRevokeResponse> {
  ErrorClass: ApiKeyErrorConstructor
  list: (includeRevoked?: boolean, offset?: number) => Promise<ApiKeyListEntry[]>
  mint: (name: string) => Promise<MintApiKeyResponse>
  revoke: (keyId: string) => Promise<TRevokeResponse>
}

export function createApiKeysApi<
  TErrorResponse extends ErrorResponseShape,
  TRevokeResponse,
>(config: ApiKeysApiConfig): ApiKeysApi<TRevokeResponse> {
  const ErrorClass = createApiKeyErrorClass(config.errorName)

  async function handleResponse<T>(response: Response): Promise<T> {
    if (!response.ok) {
      let errorData: TErrorResponse | undefined
      try {
        errorData = await response.json()
      } catch {
        // Ignore JSON parse errors
      }
      throw new ErrorClass(
        errorData?.code ?? 'UNKNOWN_ERROR',
        errorData?.message ?? `HTTP ${response.status}`,
        response.status
      )
    }
    return response.json()
  }

  async function list(includeRevoked = true, offset = 0): Promise<ApiKeyListEntry[]> {
    const response = await authenticatedFetch(
      `${getApiBase()}${config.basePath}?limit=${config.maxListLimit}&offset=${offset}&include_revoked=${includeRevoked}`
    )
    return handleResponse<ApiKeyListEntry[]>(response)
  }

  async function mint(name: string): Promise<MintApiKeyResponse> {
    const response = await authenticatedFetch(`${getApiBase()}${config.basePath}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    })
    return handleResponse<MintApiKeyResponse>(response)
  }

  async function revoke(keyId: string): Promise<TRevokeResponse> {
    const response = await authenticatedFetch(
      `${getApiBase()}${config.basePath}/${encodeURIComponent(keyId)}`,
      { method: 'DELETE' }
    )
    return handleResponse<TRevokeResponse>(response)
  }

  return { ErrorClass, list, mint, revoke }
}
