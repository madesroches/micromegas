/* eslint-disable react-refresh/only-export-components */
import React, { createContext, useContext, useEffect, useState, useCallback } from 'react'
import { getApiBase } from './api'

export interface User {
  sub: string
  email?: string
  name?: string
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
      const response = await fetch(`${getApiBase()}/auth/refresh`, {
        method: 'POST',
        credentials: 'include',
      })

      return response.ok
    } catch {
      return false
    }
  }, [])

  const checkAuth = useCallback(async (skipRefresh = false) => {
    try {
      const response = await fetch(`${getApiBase()}/auth/me`, {
        credentials: 'include',
      })

      if (response.ok) {
        const userData = await response.json()
        setUser(userData)
        setStatus('authenticated')
        setError(null)
      } else if (response.status === 401) {
        // Token expired - try to refresh if we haven't already tried
        if (!skipRefresh) {
          const refreshed = await refreshTokens()
          if (refreshed) {
            // Retry checkAuth after successful refresh (but skip refresh retry to avoid loops)
            await checkAuth(true)
          } else {
            // Refresh failed - user needs to login again
            setUser(null)
            setStatus('unauthenticated')
            setError(null)
          }
        } else {
          // Already tried refresh or explicitly skipped
          setUser(null)
          setStatus('unauthenticated')
          setError(null)
        }
      } else {
        setUser(null)
        setStatus('error')
        setError(`Server error: ${response.status}`)
      }
    } catch (err) {
      setUser(null)
      setStatus('error')
      setError(err instanceof Error ? err.message : 'Network error')
    }
  }, [refreshTokens])

  useEffect(() => {
    checkAuth()
  }, [checkAuth])

  const login = useCallback((returnUrl?: string) => {
    const currentPath = returnUrl || window.location.pathname
    const loginUrl = `${getApiBase()}/auth/login?return_url=${encodeURIComponent(currentPath)}`
    window.location.href = loginUrl
  }, [])

  const logout = useCallback(async () => {
    try {
      const response = await fetch(`${getApiBase()}/auth/logout`, {
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
      const response = await fetch(`${getApiBase()}/auth/refresh`, {
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
