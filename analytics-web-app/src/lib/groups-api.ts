/**
 * Client for the Groups admin page's REST surface -- `GET/POST/DELETE {base}/api/groups[...]`.
 * Mirrors `audience-grants-api.ts` exactly: plain `authenticatedFetch` calls, no Arrow, one
 * `handleResponse` error-decoding helper.
 */
import { authenticatedFetch, getApiBase } from './api'

export interface GroupSummary {
  name: string
  description: string | null
  member_count: number
  created_at: string
  created_by: string
}

export interface GroupMember {
  group_name: string
  member: string
  created_at: string
  created_by: string
}

export class GroupsError extends Error {
  constructor(
    public code: string,
    message: string,
    public status: number
  ) {
    super(message)
    this.name = 'GroupsError'
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
    throw new GroupsError(
      errorData?.code ?? 'UNKNOWN_ERROR',
      errorData?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
  return response.json()
}

/** `GET {base}/api/groups`. */
export async function fetchGroups(): Promise<GroupSummary[]> {
  const response = await authenticatedFetch(`${getApiBase()}/groups`)
  return handleResponse<GroupSummary[]>(response)
}

/** `POST {base}/api/groups`. `400` on a malformed name, `409` if it already exists. */
export async function createGroup(name: string, description?: string): Promise<GroupSummary> {
  const response = await authenticatedFetch(`${getApiBase()}/groups`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name, description: description || undefined }),
  })
  return handleResponse<GroupSummary>(response)
}

/** `DELETE {base}/api/groups/{name}`. `204`; `409` for `admins` or while still referenced. */
export async function deleteGroup(name: string): Promise<void> {
  const response = await authenticatedFetch(`${getApiBase()}/groups/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  })
  if (response.status === 204) return
  await handleResponse<unknown>(response)
}

/** `GET {base}/api/groups/{name}/members`. */
export async function fetchGroupMembers(name: string): Promise<GroupMember[]> {
  const response = await authenticatedFetch(
    `${getApiBase()}/groups/${encodeURIComponent(name)}/members`
  )
  return handleResponse<GroupMember[]>(response)
}

/** `POST {base}/api/groups/{name}/members`. `201` created / `200` already existed; `404` if a
 *  `group:X` member names a group that doesn't exist; `409` if it would create a cycle. */
export async function addGroupMember(
  name: string,
  member: string
): Promise<{ member: GroupMember; created: boolean }> {
  const response = await authenticatedFetch(
    `${getApiBase()}/groups/${encodeURIComponent(name)}/members`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ member }),
    }
  )
  const created = response.status === 201
  const row = await handleResponse<GroupMember>(response)
  return { member: row, created }
}

/** `DELETE {base}/api/groups/{name}/members?member=`. `204`; `404` unknown; `409` when it would
 *  remove the last row of `admins`. */
export async function removeGroupMember(name: string, member: string): Promise<void> {
  const response = await authenticatedFetch(
    `${getApiBase()}/groups/${encodeURIComponent(name)}/members?member=${encodeURIComponent(member)}`,
    { method: 'DELETE' }
  )
  if (response.status === 204) return
  await handleResponse<unknown>(response)
}

export const GROUP_NAME_PATTERN = /^[A-Za-z0-9_-]{1,255}$/
