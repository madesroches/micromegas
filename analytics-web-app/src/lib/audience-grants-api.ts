/**
 * Client for the Audience Access page's own REST surface (#1510) --
 * `GET/POST/DELETE {base}/api/audience-grants[...]`. Plain `authenticatedFetch` calls in
 * `data-sources-api.ts`'s shape, not `useStreamQuery`: unlike `QueryDenyListPage`, this page's
 * writes are fixed to this deployment's own store, so its read has to be too (see
 * `AudienceAccessPage.tsx`'s "Reading through this deployment" note) -- `fetchVisibleGrants` is
 * what the page calls to list grants, not `list_audience_grants()` and a data source.
 */
import { authenticatedFetch, getApiBase } from './api'

export type GrantAxis = 'read' | 'mint'

export interface AudienceGrant {
  audience: string
  axis: GrantAxis
  selector: string
  createdAt: Date | null // defensive, matching QueryDenyRule.createdAt
  createdBy: string
}

/** Server's raw JSON for one grant row, as `POST`/`GET .../visible` return it. */
export interface AudienceGrantResponse {
  audience: string
  axis: GrantAxis
  selector: string
  created_at: string
  created_by: string
}

export class AudienceGrantError extends Error {
  constructor(
    public code: string,
    message: string,
    public status: number
  ) {
    super(message)
    this.name = 'AudienceGrantError'
  }
}

interface ErrorResponseShape {
  code?: string
  message: string
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    let errorData: ErrorResponseShape | undefined
    try {
      errorData = await response.json()
    } catch {
      // Ignore JSON parse errors
    }
    throw new AudienceGrantError(
      errorData?.code ?? 'UNKNOWN_ERROR',
      errorData?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
  return response.json()
}

function parseDate(value: string | null | undefined): Date | null {
  if (!value) return null
  const d = new Date(value)
  return Number.isNaN(d.getTime()) ? null : d
}

function decodeGrant(row: AudienceGrantResponse): AudienceGrant {
  return {
    audience: row.audience,
    axis: row.axis,
    selector: row.selector,
    createdAt: parseDate(row.created_at),
    createdBy: row.created_by,
  }
}

/** `GET {base}/api/audience-grants/visible` -- this deployment's own store, unpaginated,
 *  decoded straight from JSON (no Arrow). What `AudienceAccessPage` actually calls to list
 *  grants. Narrows for a non-admin when the self-service knob is off (server-side); a non-2xx
 *  surfaces as {@link AudienceGrantError}. */
export async function fetchVisibleGrants(): Promise<AudienceGrant[]> {
  const response = await authenticatedFetch(`${getApiBase()}/audience-grants/visible`)
  const rows = await handleResponse<AudienceGrantResponse[]>(response)
  return rows.map(decodeGrant)
}

/** `POST {base}/api/audience-grants`. `created` is `false` when the row already existed
 *  (200, not 201). A 400/403 surfaces as {@link AudienceGrantError}. */
export async function createAudienceGrant(
  audience: string,
  axis: GrantAxis,
  selector: string
): Promise<{ grant: AudienceGrantResponse; created: boolean }> {
  const response = await authenticatedFetch(`${getApiBase()}/audience-grants`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ audience, axis, selector }),
  })
  // Read before awaiting the body -- `handleResponse` consumes it.
  const created = response.status === 201
  const grant = await handleResponse<AudienceGrantResponse>(response)
  return { grant, created }
}

/** `DELETE {base}/api/audience-grants?audience=&axis=&selector=`. Resolves on 204; 403/404
 *  surface as {@link AudienceGrantError} with the server's message. */
export async function deleteAudienceGrant(
  audience: string,
  axis: GrantAxis,
  selector: string
): Promise<void> {
  const query =
    `audience=${encodeURIComponent(audience)}` +
    `&axis=${encodeURIComponent(axis)}` +
    `&selector=${encodeURIComponent(selector)}`
  const response = await authenticatedFetch(`${getApiBase()}/audience-grants?${query}`, {
    method: 'DELETE',
  })
  if (response.status === 204) return
  // Never call `.json()` on a 204 -- axum's `NO_CONTENT` has no body to parse.
  await handleResponse<unknown>(response)
}

export interface MyAudiences {
  is_admin: boolean
  audiences: string[]
  mint_prefix: string | null
  email: string | null
  /** `"{audience}:{axis}"` for every pair the caller holds a grant on via an identity selector
   *  (i.e. the pairs the server's own hold check would accept for a create/delete) -- always
   *  empty for an admin, who doesn't need it (Share is offered everywhere on the client for an
   *  admin regardless). Ground truth for `canShareRow`: the client has no group-membership info
   *  of its own, so it can't otherwise tell "a pair I hold" apart from "a pair I can merely
   *  see" via `/visible` (which is wider -- includes pairs visible only through a `*` row or a
   *  `group:` row the caller isn't actually a member of). */
  held_pairs: string[]
  /** The caller's resolved, transitive local-group membership -- lets the page explain why a
   *  `group:` grant applies to this caller. */
  groups: string[]
}

/** `GET {base}/api/audience-grants/my-audiences`. A 403 (self-service knob off for a
 *  non-admin) is returned as {@link AudienceGrantError} so the page can hide its self-service
 *  controls instead of failing outright. */
export async function fetchMyAudiences(): Promise<MyAudiences> {
  const response = await authenticatedFetch(`${getApiBase()}/audience-grants/my-audiences`)
  return handleResponse<MyAudiences>(response)
}

export const AUDIENCE_PATTERN = /^[A-Za-z0-9_-]{1,255}$/

/** A BYTE bound (the `selector` column is `VARCHAR(255)`) -- use `TextEncoder`, not `.length`,
 *  since a `group:<id>` selector may contain multi-byte characters. */
export const MAX_SELECTOR_BYTES = 255

/** `null` when `selector` is a valid `*`/`user:<id>`/`group:<id>` selector within the byte
 *  bound; otherwise a human-readable reason, suitable for inline dialog validation text. */
export function validateSelector(selector: string): string | null {
  const isStar = selector === '*'
  const userId = selector.startsWith('user:') ? selector.slice('user:'.length) : null
  const groupId = selector.startsWith('group:') ? selector.slice('group:'.length) : null
  if (!isStar && userId === null && groupId === null) {
    return "must be '*', 'user:<id>', or 'group:<id>'"
  }
  if ((userId !== null && userId === '') || (groupId !== null && groupId === '')) {
    return 'the id after the prefix must not be empty'
  }
  if (new TextEncoder().encode(selector).length > MAX_SELECTOR_BYTES) {
    return `selector must be at most ${MAX_SELECTOR_BYTES} bytes`
  }
  return null
}
