'use client'

import React, { createContext, useContext, useEffect, useState, useCallback } from 'react'

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

const API_BASE = process.env.NODE_ENV === 'development'
  ? 'http://localhost:8000'
  : ''

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [user, setUser] = useState<User | null>(null)
  const [status, setStatus] = useState<AuthStatus>('loading')
  const [error, setError] = useState<string | null>(null)

  const checkAuth = useCallback(async () => {
    try {
      const response = await fetch(`/auth/me`, {
        credentials: 'include',
      })

      if (response.ok) {
        const userData = await response.json()
        setUser(userData)
        setStatus('authenticated')
        setError(null)
      } else if (response.status === 401) {
        setUser(null)
        setStatus('unauthenticated')
        setError(null)
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
  }, [])

  useEffect(() => {
    checkAuth()
  }, [checkAuth])

  const login = useCallback((returnUrl?: string) => {
    const currentPath = returnUrl || window.location.pathname
    const loginUrl = `/auth/login?return_url=${encodeURIComponent(currentPath)}`
    window.location.href = loginUrl
  }, [])

  const logout = useCallback(async () => {
    try {
      const response = await fetch(`/auth/logout`, {
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

  const refresh = useCallback(async (): Promise<boolean> => {
    try {
      const response = await fetch(`/auth/refresh`, {
        method: 'POST',
        credentials: 'include',
      })

      if (response.ok) {
        // Re-check auth to update user info
        await checkAuth()
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
