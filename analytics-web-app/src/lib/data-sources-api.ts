import { authenticatedFetch, getApiBase } from './api'

// Types matching the backend models

export interface DataSourceSummary {
  name: string
  is_default: boolean
}

export interface DataSourceConfig {
  url: string
}

export interface DataSource {
  name: string
  config: DataSourceConfig
  is_default: boolean
  created_by?: string
  updated_by?: string
  created_at: string
  updated_at: string
}

export interface CreateDataSourceRequest {
  name: string
  config: DataSourceConfig
  is_default?: boolean
}

export interface UpdateDataSourceRequest {
  config?: DataSourceConfig
  is_default?: boolean
}

export interface DataSourceApiErrorResponse {
  code: string
  message: string
}

export class DataSourceApiError extends Error {
  constructor(
    public code: string,
    message: string,
    public status: number
  ) {
    super(message)
    this.name = 'DataSourceApiError'
  }
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    let errorData: DataSourceApiErrorResponse | undefined
    try {
      errorData = await response.json()
    } catch {
      // Ignore JSON parse errors
    }
    throw new DataSourceApiError(
      errorData?.code ?? 'UNKNOWN_ERROR',
      errorData?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
  return response.json()
}

// Data Sources API

export async function listDataSources(): Promise<DataSourceSummary[]> {
  const response = await authenticatedFetch(`${getApiBase()}/data-sources`)
  return handleResponse<DataSourceSummary[]>(response)
}

export async function getDataSource(name: string): Promise<DataSource> {
  const response = await authenticatedFetch(
    `${getApiBase()}/data-sources/${encodeURIComponent(name)}`
  )
  return handleResponse<DataSource>(response)
}

export async function createDataSource(request: CreateDataSourceRequest): Promise<DataSource> {
  const response = await authenticatedFetch(`${getApiBase()}/data-sources`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(request),
  })
  return handleResponse<DataSource>(response)
}

export async function updateDataSource(
  name: string,
  request: UpdateDataSourceRequest
): Promise<DataSource> {
  const response = await authenticatedFetch(
    `${getApiBase()}/data-sources/${encodeURIComponent(name)}`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(request),
    }
  )
  return handleResponse<DataSource>(response)
}

export async function deleteDataSource(name: string): Promise<void> {
  const response = await authenticatedFetch(
    `${getApiBase()}/data-sources/${encodeURIComponent(name)}`,
    { method: 'DELETE' }
  )
  if (!response.ok) {
    let errorData: DataSourceApiErrorResponse | undefined
    try {
      errorData = await response.json()
    } catch {
      // Ignore JSON parse errors
    }
    throw new DataSourceApiError(
      errorData?.code ?? 'UNKNOWN_ERROR',
      errorData?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
}
