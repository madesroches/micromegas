import { GenerateTraceRequest, ProgressUpdate, BinaryStartMarker } from '@/types'
import { getConfig } from './config'

export function getApiBase(): string {
  return `${getConfig().basePath}/api`
}

export function getAuthBase(): string {
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

  // Set timestamp immediately to prevent race conditions with concurrent calls
  lastRefreshAttempt = now

  // If a refresh is already in progress, wait for it
  if (refreshPromise) {
    return refreshPromise
  }

  refreshPromise = (async () => {
    try {
      const response = await fetch(`${getAuthBase()}/auth/refresh`, {
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
    // 401 check handles the case where token refresh failed - user must re-authenticate
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

  // Find the byte offset right after the binary_start line's \n in raw bytes.
  // With gzip compression, the browser may merge the binary_start JSON line
  // and binary data into a single decompressed chunk, so we need to extract
  // the trailing binary data rather than discarding the whole chunk.
  function findBinaryDataOffset(data: Uint8Array): number {
    const marker = [0x62,0x69,0x6e,0x61,0x72,0x79,0x5f,0x73,0x74,0x61,0x72,0x74] // "binary_start"
    outer: for (let i = 0; i <= data.length - marker.length; i++) {
      for (let j = 0; j < marker.length; j++) {
        if (data[i + j] !== marker[j]) continue outer
      }
      for (let k = i + marker.length; k < data.length; k++) {
        if (data[k] === 0x0a) return k + 1
      }
      return data.length
    }
    return -1
  }

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
              // Extract any binary data trailing after the marker in this chunk
              const binaryOffset = findBinaryDataOffset(value)
              if (binaryOffset >= 0 && binaryOffset < value.length) {
                const trailing = value.slice(binaryOffset)
                chunks.push(trailing)
                bytesReceived += trailing.length
              }
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

        continue
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