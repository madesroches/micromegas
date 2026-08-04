/* eslint-disable react-refresh/only-export-components */
import React, { createContext, useContext, useEffect, useState, useCallback } from 'react'
import { getAuthBase } from './api'
import { navigateTo } from './navigation'

export interface User {
  sub: string
  email?: string
  name?: string
  is_admin?: boolean
}

export type AuthStatus = 'loading' | 'authenticated' | 'unauthenticated' | 'error'

interface AuthContextType {
  user: User | null
  status: AuthStatus
  error: string | null
  login: (returnUrl?: string) => void
  logout: () => Promise<void>
  refresh: () => Promise<boolean>
}

const AuthContext = createContext<AuthContextType | undefined>(undefined)

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [user, setUser] = useState<User | null>(null)
  const [status, setStatus] = useState<AuthStatus>('loading')
  const [error, setError] = useState<string | null>(null)

  // Internal function to refresh tokens without triggering checkAuth
  const refreshTokens = useCallback(async (): Promise<boolean> => {
    try {
      const response = await fetch(`${getAuthBase()}/auth/refresh`, {
        method: 'POST',
        credentials: 'include',
      })

      return response.ok
    } catch {
      return false
    }
  }, [])

  // Retry-once loop instead of self-recursion: allowRefresh starts true and is
  // cleared after one refresh attempt, so `continue` here runs at most twice.
  const checkAuth = useCallback(async (skipRefresh = false) => {
    let allowRefresh = !skipRefresh
    while (true) {
      try {
        const response = await fetch(`${getAuthBase()}/auth/me`, {
          credentials: 'include',
        })

        if (response.ok) {
          const userData = await response.json()
          setUser(userData)
          setStatus('authenticated')
          setError(null)
          return
        }

        if (response.status === 401 && allowRefresh) {
          const refreshed = await refreshTokens()
          if (refreshed) {
            // Retry after successful refresh, but don't allow a second refresh attempt
            allowRefresh = false
            continue
          }
        }

        if (response.status === 401) {
          // Refresh failed, wasn't attempted, or was already tried - user needs to login again
          setUser(null)
          setStatus('unauthenticated')
          setError(null)
        } else {
          setUser(null)
          setStatus('error')
          setError(`Server error: ${response.status}`)
        }
        return
      } catch (err) {
        setUser(null)
        setStatus('error')
        setError(err instanceof Error ? err.message : 'Network error')
        return
      }
    }
  }, [refreshTokens])

  useEffect(() => {
    checkAuth()
  }, [checkAuth])

  const login = useCallback((returnUrl?: string) => {
    const currentPath = returnUrl || window.location.pathname
    const loginUrl = `${getAuthBase()}/auth/login?return_url=${encodeURIComponent(currentPath)}`
    navigateTo(loginUrl)
  }, [])

  const logout = useCallback(async () => {
    try {
      const response = await fetch(`${getAuthBase()}/auth/logout`, {
        method: 'POST',
        credentials: 'include',
      })

      if (response.ok) {
        setUser(null)
        setStatus('unauthenticated')
        setError(null)
      } else {
        throw new Error(`Logout failed: ${response.status}`)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Logout failed')
      throw err
    }
  }, [])

  // Public refresh function that also updates auth state
  const refresh = useCallback(async (): Promise<boolean> => {
    try {
      const response = await fetch(`${getAuthBase()}/auth/refresh`, {
        method: 'POST',
        credentials: 'include',
      })

      if (response.ok) {
        // Re-check auth to update user info
        await checkAuth(true) // Skip automatic refresh retry
        return true
      } else {
        setUser(null)
        setStatus('unauthenticated')
        return false
      }
    } catch {
      setUser(null)
      setStatus('unauthenticated')
      return false
    }
  }, [checkAuth])

  const value: AuthContextType = {
    user,
    status,
    error,
    login,
    logout,
    refresh,
  }

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>
}

export function useAuth() {
  const context = useContext(AuthContext)
  if (context === undefined) {
    throw new Error('useAuth must be used within an AuthProvider')
  }
  return context
}
