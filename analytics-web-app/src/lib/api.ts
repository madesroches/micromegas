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

