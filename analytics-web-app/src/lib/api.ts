import { GenerateTraceRequest, ProgressUpdate, BinaryStartMarker, SqlQueryRequest, SqlQueryResponse, SqlQueryError, SqlRow } from '@/types'
import { getConfig } from './config'

function getApiBase(): string {
  return getConfig().basePath
}

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

// Token refresh state to prevent multiple concurrent refresh attempts
let refreshPromise: Promise<boolean> | null = null
let lastRefreshAttempt = 0
const REFRESH_COOLDOWN_MS = 5000 // Don't attempt refresh more than once every 5 seconds

async function refreshToken(): Promise<boolean> {
  const now = Date.now()

  // Prevent refresh loops: if we just attempted a refresh, don't try again
  if (now - lastRefreshAttempt < REFRESH_COOLDOWN_MS) {
    return false
  }

  // If a refresh is already in progress, wait for it
  if (refreshPromise) {
    return refreshPromise
  }

  lastRefreshAttempt = now

  refreshPromise = (async () => {
    try {
      const response = await fetch(`${getApiBase()}/auth/refresh`, {
        method: 'POST',
        credentials: 'include',
      })
      return response.ok
    } catch {
      return false
    } finally {
      refreshPromise = null
    }
  })()

  return refreshPromise
}

/**
 * Fetch wrapper that automatically refreshes the token on 401 responses.
 * If the token refresh succeeds, the original request is retried once.
 * Includes cooldown to prevent refresh loops.
 */
export async function authenticatedFetch(
  input: RequestInfo | URL,
  init?: RequestInit
): Promise<Response> {
  const response = await fetch(input, {
    ...init,
    credentials: 'include',
  })

  if (response.status === 401) {
    const refreshed = await refreshToken()
    if (refreshed) {
      // Retry the original request with refreshed token (using plain fetch, not recursive)
      return fetch(input, {
        ...init,
        credentials: 'include',
      })
    }
  }

  return response
}

/** Convert SQL query response to array of row objects */
export function toRowObjects(result: SqlQueryResponse): SqlRow[] {
  return result.rows.map(row =>
    Object.fromEntries(result.columns.map((col, i) => [col, row[i]]))
  )
}

export interface GenerateTraceOptions {
  /** If true, return ArrayBuffer instead of downloading */
  returnBuffer?: boolean
}

/**
 * Generate a Perfetto trace for a process.
 *
 * @param processId - The process ID to generate trace for
 * @param request - Trace generation request parameters
 * @param onProgress - Optional callback for progress updates
 * @param options - Optional options for controlling output behavior
 * @returns ArrayBuffer if returnBuffer is true, void otherwise (triggers download)
 */
export async function generateTrace(
  processId: string,
  request: GenerateTraceRequest,
  onProgress?: (update: ProgressUpdate) => void,
  options?: GenerateTraceOptions
): Promise<ArrayBuffer | void> {
  const response = await authenticatedFetch(`${getApiBase()}/perfetto/${processId}/generate`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
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
  let bytesReceived = 0

  // eslint-disable-next-line no-constant-condition
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
            bytesReceived += value.length
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
      // Collect binary chunks and track download progress
      chunks.push(value)
      bytesReceived += value.length

      // Report download progress
      if (onProgress) {
        const mbReceived = (bytesReceived / (1024 * 1024)).toFixed(1)
        onProgress({
          type: 'progress',
          message: `Downloading trace data... ${mbReceived} MB received`
        })
      }
    }
  }

  // Combine chunks into a single buffer
  const totalLength = chunks.reduce((acc, chunk) => acc + chunk.length, 0)
  const combined = new Uint8Array(totalLength)
  let offset = 0
  for (const chunk of chunks) {
    combined.set(chunk, offset)
    offset += chunk.length
  }

  // Return buffer or download based on options
  if (options?.returnBuffer) {
    return combined.buffer
  }

  // Default: download the file
  const blob = new Blob([combined], { type: 'application/octet-stream' })
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
  const response = await authenticatedFetch(`${getApiBase()}/query`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
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