import { GenerateTraceRequest, HealthCheck, ProgressUpdate, BinaryStartMarker, SqlQueryRequest, SqlQueryResponse, SqlQueryError, SqlRow } from '@/types'

const API_BASE = process.env.NODE_ENV === 'development' ? 'http://localhost:8000/analyticsweb' : '/analyticsweb'

export interface ApiError {
  type: string
  message: string
  details?: string
}

export class ApiErrorException extends Error {
  constructor(public apiError: ApiError) {
    super(apiError.message)
    this.name = 'ApiErrorException'
  }
}

export class AuthenticationError extends Error {
  constructor(message: string = 'Authentication required') {
    super(message)
    this.name = 'AuthenticationError'
  }
}

/** Convert SQL query response to array of row objects */
export function toRowObjects(result: SqlQueryResponse): SqlRow[] {
  return result.rows.map(row =>
    Object.fromEntries(result.columns.map((col, i) => [col, row[i]]))
  )
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    if (response.status === 401) {
      throw new AuthenticationError()
    }
    try {
      const errorData = await response.json()
      if (errorData.error) {
        throw new ApiErrorException(errorData.error as ApiError)
      }
    } catch (parseError) {
      // If we can't parse the error response, fall back to generic error
      if (parseError instanceof ApiErrorException) {
        throw parseError
      }
      if (parseError instanceof AuthenticationError) {
        throw parseError
      }
    }
    throw new Error(`HTTP ${response.status}: ${response.statusText}`)
  }
  return response.json()
}

export async function fetchHealthCheck(): Promise<HealthCheck> {
  const response = await fetch(`${API_BASE}/health`, {
    credentials: 'include',
  })
  return handleResponse<HealthCheck>(response)
}

export async function generateTrace(
  processId: string,
  request: GenerateTraceRequest,
  onProgress?: (update: ProgressUpdate) => void
): Promise<void> {
  const response = await fetch(`${API_BASE}/perfetto/${processId}/generate`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    credentials: 'include',
    body: JSON.stringify(request)
  })

  if (!response.ok) {
    if (response.status === 401) {
      throw new AuthenticationError()
    }
    try {
      const errorData = await response.json()
      if (errorData.error) {
        throw new ApiErrorException(errorData.error as ApiError)
      }
    } catch (parseError) {
      if (parseError instanceof ApiErrorException) {
        throw parseError
      }
      if (parseError instanceof AuthenticationError) {
        throw parseError
      }
    }
    throw new Error(`Failed to generate trace: HTTP ${response.status}`)
  }

  if (!response.body) {
    throw new Error('No response body')
  }

  const reader = response.body.getReader()
  const chunks: Uint8Array[] = []
  let progressComplete = false

  while (true) {
    const { done, value } = await reader.read()
    if (done) break

    if (!progressComplete) {
      // Try to parse as JSON progress update
      try {
        const chunk = new TextDecoder().decode(value)
        const lines = chunk.split('\n').filter(line => line.trim())

        for (const line of lines) {
          try {
            const update = JSON.parse(line) as ProgressUpdate | BinaryStartMarker

            if (update.type === 'progress' && onProgress) {
              onProgress(update as ProgressUpdate)
              continue
            } else if (update.type === 'binary_start') {
              progressComplete = true
              break
            }
          } catch {
            // Not JSON, must be binary data
            progressComplete = true
            chunks.push(value)
            break
          }
        }

        if (progressComplete && chunks.length === 0) continue
      } catch {
        // Not JSON, must be binary data
        progressComplete = true
      }
    }

    if (progressComplete) {
      // Collect binary chunks
      chunks.push(value)
    }
  }

  // Create blob and download
  const blob = new Blob(chunks as BlobPart[], { type: 'application/octet-stream' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `perfetto-${processId}-${Date.now()}.pb`
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}

export async function executeSqlQuery(request: SqlQueryRequest): Promise<SqlQueryResponse> {
  const response = await fetch(`${API_BASE}/query`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    credentials: 'include',
    body: JSON.stringify(request),
  })

  if (!response.ok) {
    if (response.status === 401) {
      throw new AuthenticationError()
    }
    if (response.status === 403) {
      const errorData = await response.json() as SqlQueryError
      throw new Error(errorData.details || errorData.error)
    }
    if (response.status === 400) {
      const errorData = await response.json() as SqlQueryError
      throw new Error(errorData.details || errorData.error)
    }
    throw new Error(`HTTP ${response.status}: ${response.statusText}`)
  }

  return response.json()
}