import { authenticatedFetch, getApiBase } from './api'
import { ApiErrorResponse, ScreenApiError } from './screens-api'

export interface FolderInfo {
  path: string
  screen_count: number
  subfolder_count: number
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

export async function listFolders(): Promise<FolderInfo[]> {
  const response = await authenticatedFetch(`${getApiBase()}/folders`)
  return handleResponse<FolderInfo[]>(response)
}

export async function createFolder(path: string): Promise<{ path: string }> {
  const response = await authenticatedFetch(`${getApiBase()}/folders`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ path }),
  })
  return handleResponse<{ path: string }>(response)
}

export async function moveFolder(path: string, newPath: string): Promise<{ path: string }> {
  const response = await authenticatedFetch(`${getApiBase()}/folders`, {
    method: 'PUT',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ path, new_path: newPath }),
  })
  return handleResponse<{ path: string }>(response)
}

export async function deleteFolder(path: string): Promise<void> {
  const response = await authenticatedFetch(
    `${getApiBase()}/folders?path=${encodeURIComponent(path)}`,
    { method: 'DELETE' }
  )
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
