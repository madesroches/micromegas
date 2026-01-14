import { authenticatedFetch } from './api'
import { getConfig } from './config'

function getApiBase(): string {
  return getConfig().basePath
}

// Types matching the backend models

export interface MetricsOptions {
  scale_mode?: 'p99' | 'max'
}

export interface ScreenConfig {
  sql: string
  variables?: ScreenVariable[]
  metrics_options?: MetricsOptions
}

export interface ScreenVariable {
  name: string
  default_value?: string
}

export interface Screen {
  name: string
  screen_type: ScreenTypeName
  config: ScreenConfig
  created_by?: string
  updated_by?: string
  created_at: string
  updated_at: string
}

export type ScreenTypeName = 'process_list' | 'metrics' | 'log'

export interface ScreenTypeInfo {
  name: ScreenTypeName
  display_name: string
  icon: string
  description: string
}

export interface CreateScreenRequest {
  name: string
  screen_type: ScreenTypeName
  config: ScreenConfig
}

export interface UpdateScreenRequest {
  config: ScreenConfig
}

export interface ApiErrorResponse {
  code: string
  message: string
}

export class ScreenApiError extends Error {
  constructor(
    public code: string,
    message: string,
    public status: number
  ) {
    super(message)
    this.name = 'ScreenApiError'
  }
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    let errorData: ApiErrorResponse | undefined
    try {
      errorData = await response.json()
    } catch {
      // Ignore JSON parse errors
    }
    throw new ScreenApiError(
      errorData?.code ?? 'UNKNOWN_ERROR',
      errorData?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
  return response.json()
}

// Screen Types API

export async function getScreenTypes(): Promise<ScreenTypeInfo[]> {
  const response = await authenticatedFetch(`${getApiBase()}/screen-types`)
  return handleResponse<ScreenTypeInfo[]>(response)
}

export async function getDefaultConfig(typeName: ScreenTypeName): Promise<ScreenConfig> {
  const response = await authenticatedFetch(`${getApiBase()}/screen-types/${typeName}/default`)
  return handleResponse<ScreenConfig>(response)
}

// Screens CRUD API

export async function listScreens(): Promise<Screen[]> {
  const response = await authenticatedFetch(`${getApiBase()}/screens`)
  return handleResponse<Screen[]>(response)
}

export async function getScreen(name: string): Promise<Screen> {
  const response = await authenticatedFetch(`${getApiBase()}/screens/${encodeURIComponent(name)}`)
  return handleResponse<Screen>(response)
}

export async function createScreen(request: CreateScreenRequest): Promise<Screen> {
  const response = await authenticatedFetch(`${getApiBase()}/screens`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  })
  return handleResponse<Screen>(response)
}

export async function updateScreen(name: string, request: UpdateScreenRequest): Promise<Screen> {
  const response = await authenticatedFetch(`${getApiBase()}/screens/${encodeURIComponent(name)}`, {
    method: 'PUT',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  })
  return handleResponse<Screen>(response)
}

export async function deleteScreen(name: string): Promise<void> {
  const response = await authenticatedFetch(`${getApiBase()}/screens/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  })
  if (!response.ok) {
    let errorData: ApiErrorResponse | undefined
    try {
      errorData = await response.json()
    } catch {
      // Ignore JSON parse errors
    }
    throw new ScreenApiError(
      errorData?.code ?? 'UNKNOWN_ERROR',
      errorData?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
}
