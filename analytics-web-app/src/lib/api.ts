import { GenerateTraceRequest, ProgressUpdate, BinaryStartMarker } from '@/types'
import * as lz4 from 'lz4js'
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
  const decompressedChunks: Uint8Array[] = []
  let progressComplete = false
  let decompressedSize = 0
  // Small buffer for reassembling length-prefixed lz4 frames across stream chunks
  let frameBuf = new Uint8Array(0)

  // Parse complete length-prefixed lz4 frames from frameBuf, decompress each
  // immediately, and discard the compressed data so only decompressed output
  // accumulates in memory.
  function drainFrames() {
    let pos = 0
    while (pos + 4 <= frameBuf.length) {
      const frameLen =
        (frameBuf[pos] << 24) |
        (frameBuf[pos + 1] << 16) |
        (frameBuf[pos + 2] << 8) |
        frameBuf[pos + 3]
      if (pos + 4 + frameLen > frameBuf.length) break
      const frame = frameBuf.subarray(pos + 4, pos + 4 + frameLen)
      const decompressed = lz4.decompress(frame) as Uint8Array<ArrayBuffer>
      decompressedChunks.push(decompressed)
      decompressedSize += decompressed.length
      pos += 4 + frameLen
    }
    frameBuf = pos > 0 ? frameBuf.slice(pos) : frameBuf
  }

  // Find the byte offset in raw data right after the binary_start line's \n.
  // All JSON is ASCII so byte positions match character positions for that portion.
  function findBinaryDataOffset(data: Uint8Array): number {
    // Search for ASCII "binary_start" in raw bytes
    const marker = [0x62,0x69,0x6e,0x61,0x72,0x79,0x5f,0x73,0x74,0x61,0x72,0x74] // "binary_start"
    outer: for (let i = 0; i <= data.length - marker.length; i++) {
      for (let j = 0; j < marker.length; j++) {
        if (data[i + j] !== marker[j]) continue outer
      }
      // Found marker, find next \n (0x0a) after it
      for (let k = i + marker.length; k < data.length; k++) {
        if (data[k] === 0x0a) return k + 1
      }
      return data.length
    }
    return -1
  }

  function appendToFrameBuf(data: Uint8Array<ArrayBufferLike>) {
    if (frameBuf.length === 0) {
      frameBuf = new Uint8Array(data)
    } else {
      const newBuf = new Uint8Array(frameBuf.length + data.length)
      newBuf.set(frameBuf)
      newBuf.set(data, frameBuf.length)
      frameBuf = newBuf
    }
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
                appendToFrameBuf(value.slice(binaryOffset))
                drainFrames()
              }
              break
            }
          } catch {
            // Not JSON, must be binary data
            progressComplete = true
            appendToFrameBuf(value)
            drainFrames()
            break
          }
        }

        // All data from this chunk has been handled above (JSON parsed,
        // or binary data extracted via findBinaryDataOffset / catch handler).
        // Always skip the raw append below.
        continue
      } catch {
        // TextDecoder failed entirely - raw binary data
        progressComplete = true
      }
    }

    if (progressComplete) {
      // Append to frame buffer and decompress any complete frames
      appendToFrameBuf(value)
      drainFrames()

      // Report download progress based on decompressed size
      if (onProgress) {
        const mbDecompressed = (decompressedSize / (1024 * 1024)).toFixed(1)
        onProgress({
          type: 'progress',
          message: `Downloading trace data... ${mbDecompressed} MB decompressed`
        })
      }
    }
  }

  // Combine decompressed chunks into a single buffer
  const combined = new Uint8Array(decompressedSize)
  let offset = 0
  for (const chunk of decompressedChunks) {
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