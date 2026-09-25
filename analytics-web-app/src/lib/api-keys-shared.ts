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
  /** The write audience an ingestion key is bound to (#1372). Analytics rows never carry one. */
  audience?: string
  /** Empty = unrestricted. The server COALESCEs NULL to [], so this is always present. */
  allowed_cidrs: string[]
}

export interface MintApiKeyResponse {
  key_id: string
  name: string
  created_at: string
  /** The cleartext key, returned exactly once. Never persisted client-side. */
  key: string
  /** The audience the minted key was stamped with (#1372). Analytics keys never carry one. */
  audience?: string
  /** `true` only when this mint call also claimed a brand-new audience for the caller --
   *  writing their own `read`+`mint` grant rows in the same request (#1510, AbAC Stage 6c).
   *  `false`/absent otherwise; ingestion-only, analytics keys never carry it. */
  claimed?: boolean
}

export interface MintApiKeyOptions {
  audience?: string
  allowed_cidrs?: string[]
}

export interface SetAllowlistResponse {
  allowed_cidrs: string[]
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
}

export interface ApiKeysApi<TRevokeResponse> {
  ErrorClass: ApiKeyErrorConstructor
  list: (includeRevoked: boolean, offset: number, limit: number) => Promise<ApiKeyListEntry[]>
  mint: (name: string, options?: MintApiKeyOptions) => Promise<MintApiKeyResponse>
  revoke: (keyId: string) => Promise<TRevokeResponse>
  setAllowlist: (keyId: string, allowedCidrs: string[]) => Promise<SetAllowlistResponse>
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

  // `limit` is the caller's page size rather than a module constant: the admin
  // page owns the value (it also drives the offset arithmetic and the
  // "is there a next page?" row-count comparison), so a single number has to
  // reach both the query string and the paging UI. All three args are
  // required — a defaulted `limit` here could silently disagree with the page
  // size the UI is comparing against.
  async function list(
    includeRevoked: boolean,
    offset: number,
    limit: number
  ): Promise<ApiKeyListEntry[]> {
    const response = await authenticatedFetch(
      `${getApiBase()}${config.basePath}?limit=${limit}&offset=${offset}&include_revoked=${includeRevoked}`
    )
    return handleResponse<ApiKeyListEntry[]>(response)
  }

  async function mint(name: string, options: MintApiKeyOptions = {}): Promise<MintApiKeyResponse> {
    const response = await authenticatedFetch(`${getApiBase()}${config.basePath}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      // `audience`/`allowed_cidrs` are only ever unset for callers that don't ask for one
      // (analytics keys never carry an audience; most mints carry no allowlist).
      // `JSON.stringify` drops an `undefined` value entirely, so that case omits the field
      // rather than sending `null`.
      body: JSON.stringify({
        name,
        audience: options.audience,
        allowed_cidrs: options.allowed_cidrs,
      }),
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

  async function setAllowlist(
    keyId: string,
    allowedCidrs: string[]
  ): Promise<SetAllowlistResponse> {
    const response = await authenticatedFetch(
      `${getApiBase()}${config.basePath}/${encodeURIComponent(keyId)}/allowlist`,
      {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ allowed_cidrs: allowedCidrs }),
      }
    )
    return handleResponse<SetAllowlistResponse>(response)
  }

  return { ErrorClass, list, mint, revoke, setAllowlist }
}

/**
 * Splits free-text input on newlines, commas, and whitespace, and drops blanks. No validation:
 * the server's `IpAllowlist::parse` is the one validator, and its 400 message is shown to the
 * user unchanged.
 */
export function parseAllowlistInput(text: string): string[] {
  return text
    .split(/[\s,]+/)
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0)
}
