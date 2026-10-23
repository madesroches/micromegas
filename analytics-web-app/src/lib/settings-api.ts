/**
 * Client for the Settings admin page's REST surface -- `GET/PUT/DELETE {base}/api/settings[...]`.
 * Same shape as `groups-api.ts`: plain `authenticatedFetch` calls and one `handleResponse`
 * error-decoding helper.
 */
import { authenticatedFetch, getApiBase } from './api'

export type SettingKind = 'bool' | 'count' | 'name_list'

export type SettingValue = boolean | number | string[]

export interface SettingEntry {
  key: string
  kind: SettingKind
  env_var: string
  default: SettingValue
  description: string
  /** `null` when no row is stored: the services fall back to the env var, then the default. */
  value: SettingValue | null
  updated_at: string | null
  updated_by: string | null
}

export class SettingsError extends Error {
  constructor(
    public code: string,
    message: string,
    public status: number
  ) {
    super(message)
    this.name = 'SettingsError'
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
    throw new SettingsError(
      errorData?.code ?? 'UNKNOWN_ERROR',
      errorData?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
  return response.json()
}

/** `GET {base}/api/settings`. One entry per registered setting. */
export async function fetchSettings(): Promise<SettingEntry[]> {
  const response = await authenticatedFetch(`${getApiBase()}/settings`)
  return handleResponse<SettingEntry[]>(response)
}

/** `PUT {base}/api/settings/{key}`. `404` on an unknown key, `400` on an invalid value. */
export async function setSetting(key: string, value: SettingValue): Promise<SettingEntry> {
  const response = await authenticatedFetch(
    `${getApiBase()}/settings/${encodeURIComponent(key)}`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ value }),
    }
  )
  return handleResponse<SettingEntry>(response)
}

/** `DELETE {base}/api/settings/{key}`. `204`; the key reverts to its env var or default. */
export async function resetSetting(key: string): Promise<void> {
  const response = await authenticatedFetch(
    `${getApiBase()}/settings/${encodeURIComponent(key)}`,
    { method: 'DELETE' }
  )
  if (response.status === 204) return
  await handleResponse<unknown>(response)
}
