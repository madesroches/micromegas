import { render, screen, waitFor } from '@testing-library/react'
import { act } from 'react'
import { AuthProvider, useAuth } from '../auth'

// Mock fetch globally
global.fetch = jest.fn()

describe('AuthProvider', () => {
  beforeEach(() => {
    jest.clearAllMocks()
    ;(global.fetch as jest.Mock).mockReset()
  })

  afterEach(() => {
    jest.clearAllMocks()
  })

  const TestComponent = () => {
    const { status, user, error } = useAuth()
    return (
      <div>
        <div data-testid="status">{status}</div>
        <div data-testid="user">{user ? JSON.stringify(user) : 'null'}</div>
        <div data-testid="error">{error || 'null'}</div>
      </div>
    )
  }

  it('should initialize with loading status', () => {
    (global.fetch as jest.Mock).mockImplementation(
      () => new Promise(() => {}) // Never resolves
    )

    render(
      <AuthProvider>
        <TestComponent />
      </AuthProvider>
    )

    expect(screen.getByTestId('status')).toHaveTextContent('loading')
    expect(screen.getByTestId('user')).toHaveTextContent('null')
  })

  it('should load authenticated user on mount', async () => {
    (global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        sub: 'user123',
        email: 'test@example.com',
        name: 'Test User',
      }),
    })

    render(
      <AuthProvider>
        <TestComponent />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByTestId('status')).toHaveTextContent('authenticated')
    })

    const user = JSON.parse(screen.getByTestId('user').textContent || '{}')
    expect(user.sub).toBe('user123')
    expect(user.email).toBe('test@example.com')
    expect(user.name).toBe('Test User')

    expect(global.fetch).toHaveBeenCalledWith(
      '/auth/me',
      expect.objectContaining({
        credentials: 'include',
      })
    )
  })

  it('should handle unauthenticated status (401) and attempt refresh', async () => {
    (global.fetch as jest.Mock)
      // First call to /auth/me returns 401
      .mockResolvedValueOnce({
        ok: false,
        status: 401,
      })
      // Second call to /auth/refresh fails (no valid refresh token)
      .mockResolvedValueOnce({
        ok: false,
        status: 401,
      })

    render(
      <AuthProvider>
        <TestComponent />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByTestId('status')).toHaveTextContent('unauthenticated')
    })

    expect(screen.getByTestId('user')).toHaveTextContent('null')
    expect(screen.getByTestId('error')).toHaveTextContent('null')

    // Verify that refresh was attempted
    expect(global.fetch).toHaveBeenCalledWith(
      '/auth/refresh',
      expect.objectContaining({
        method: 'POST',
        credentials: 'include',
      })
    )
  })

  it('should automatically refresh expired token and retry auth check', async () => {
    (global.fetch as jest.Mock)
      // First call to /auth/me returns 401 (expired token)
      .mockResolvedValueOnce({
        ok: false,
        status: 401,
      })
      // Second call to /auth/refresh succeeds
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
      })
      // Third call to /auth/me succeeds with new token
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          sub: 'user123',
          email: 'test@example.com',
          name: 'Test User',
        }),
      })

    render(
      <AuthProvider>
        <TestComponent />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByTestId('status')).toHaveTextContent('authenticated')
    })

    const user = JSON.parse(screen.getByTestId('user').textContent || '{}')
    expect(user.sub).toBe('user123')
    expect(user.email).toBe('test@example.com')

    // Verify the full flow: /auth/me (401) -> /auth/refresh (200) -> /auth/me (200)
    expect(global.fetch).toHaveBeenCalledTimes(3)
    expect(global.fetch).toHaveBeenNthCalledWith(
      1,
      '/auth/me',
      expect.any(Object)
    )
    expect(global.fetch).toHaveBeenNthCalledWith(
      2,
      '/auth/refresh',
      expect.objectContaining({
        method: 'POST',
        credentials: 'include',
      })
    )
    expect(global.fetch).toHaveBeenNthCalledWith(
      3,
      '/auth/me',
      expect.any(Object)
    )
  })

  it('should handle service unavailable error (500)', async () => {
    (global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: false,
      status: 500,
    })

    render(
      <AuthProvider>
        <TestComponent />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByTestId('status')).toHaveTextContent('error')
    })

    expect(screen.getByTestId('error')).toHaveTextContent('Server error: 500')
  })

  it('should handle network error', async () => {
    (global.fetch as jest.Mock).mockRejectedValueOnce(
      new Error('Network error')
    )

    render(
      <AuthProvider>
        <TestComponent />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByTestId('status')).toHaveTextContent('error')
    })

    expect(screen.getByTestId('error')).toHaveTextContent('Network error')
  })

  it('should handle invalid JSON response', async () => {
    (global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => {
        throw new Error('Invalid JSON')
      },
    })

    render(
      <AuthProvider>
        <TestComponent />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByTestId('status')).toHaveTextContent('error')
    })
  })
})

describe('useAuth hook', () => {
  beforeEach(() => {
    jest.clearAllMocks()
    ;(global.fetch as jest.Mock).mockReset()
  })

  const TestLoginComponent = () => {
    const { login } = useAuth()
    return <button onClick={() => login('/dashboard')}>Login</button>
  }

  const TestLogoutComponent = () => {
    const { logout, status } = useAuth()
    return (
      <div>
        <div data-testid="status">{status}</div>
        <button onClick={logout}>Logout</button>
      </div>
    )
  }

  const TestRefreshComponent = () => {
    const { refresh } = useAuth()
    return <button onClick={refresh}>Refresh</button>
  }

  it('should throw error when used outside AuthProvider', () => {
    // Suppress console.error for this test
    const originalError = console.error
    console.error = jest.fn()

    expect(() => {
      render(<TestLoginComponent />)
    }).toThrow('useAuth must be used within an AuthProvider')

    console.error = originalError
  })

  it('should call login with return URL', async () => {
    (global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({ sub: 'user123', email: 'test@example.com' }),
    })

    const { container } = render(
      <AuthProvider>
        <TestLoginComponent />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalledWith(
        '/auth/me',
        expect.any(Object)
      )
    })

    const button = container.querySelector('button')
    act(() => {
      button?.click()
    })

    await waitFor(() => {
      expect(window.location.href).toContain('/auth/login')
    })
  })

  it('should call logout endpoint', async () => {
    (global.fetch as jest.Mock)
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ sub: 'user123', email: 'test@example.com' }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
      })

    const { container } = render(
      <AuthProvider>
        <TestLogoutComponent />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByTestId('status')).toHaveTextContent('authenticated')
    })

    const button = container.querySelector('button')
    await act(async () => {
      button?.click()
      // Wait for logout promise to resolve
      await new Promise(resolve => setTimeout(resolve, 10))
    })

    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalledWith(
        '/auth/logout',
        expect.objectContaining({
          method: 'POST',
          credentials: 'include',
        })
      )
    })

    await waitFor(() => {
      expect(screen.getByTestId('status')).toHaveTextContent('unauthenticated')
    })
  })

  it('should call refresh endpoint', async () => {
    (global.fetch as jest.Mock)
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ sub: 'user123', email: 'test@example.com' }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ sub: 'user123', email: 'test@example.com' }),
      })

    const { container } = render(
      <AuthProvider>
        <TestRefreshComponent />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalledTimes(1)
    })

    const button = container.querySelector('button')
    await act(async () => {
      button?.click()
      // Wait for refresh to complete
      await new Promise(resolve => setTimeout(resolve, 10))
    })

    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalledWith(
        '/auth/refresh',
        expect.objectContaining({
          method: 'POST',
          credentials: 'include',
        })
      )
    })

    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalledTimes(3)
    })
  })

  it('should handle logout failure by setting error state', async () => {
    (global.fetch as jest.Mock)
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ sub: 'user123', email: 'test@example.com' }),
      })
      .mockResolvedValueOnce({
        ok: false,
        status: 500,
      })

    // Suppress console.error for expected error
    const originalError = console.error
    console.error = jest.fn()

    const TestLogoutErrorComponent = () => {
      const { logout, status, error } = useAuth()
      const handleLogout = async () => {
        try {
          await logout()
        } catch (err) {
          // Error is caught and handled
        }
      }
      return (
        <div>
          <div data-testid="status">{status}</div>
          <div data-testid="error">{error || 'null'}</div>
          <button onClick={handleLogout}>Logout</button>
        </div>
      )
    }

    const { container } = render(
      <AuthProvider>
        <TestLogoutErrorComponent />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByTestId('status')).toHaveTextContent('authenticated')
    })

    const button = container.querySelector('button')
    await act(async () => {
      button?.click()
      await new Promise(resolve => setTimeout(resolve, 50))
    })

    console.error = originalError

    // Logout failed, so status remains authenticated and error is set
    expect(screen.getByTestId('status')).toHaveTextContent('authenticated')
    await waitFor(() => {
      expect(screen.getByTestId('error')).toHaveTextContent('Logout failed: 500')
    })
  })

  it('should handle refresh failure', async () => {
    (global.fetch as jest.Mock)
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ sub: 'user123', email: 'test@example.com' }),
      })
      .mockResolvedValueOnce({
        ok: false,
        status: 401,
      })

    const { container } = render(
      <AuthProvider>
        <TestRefreshComponent />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalledTimes(1)
    })

    const button = container.querySelector('button')

    // Suppress console.error for expected error
    const originalError = console.error
    console.error = jest.fn()

    await act(async () => {
      try {
        button?.click()
        await new Promise(resolve => setTimeout(resolve, 10))
      } catch (err) {
        // Expected to fail
      }
    })

    console.error = originalError

    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalledWith(
        '/auth/refresh',
        expect.any(Object)
      )
    })
  })
})
