import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { UserMenu } from '../UserMenu'
import { AuthProvider } from '@/lib/auth'

// Mock fetch globally
global.fetch = jest.fn()

describe('UserMenu', () => {
  beforeEach(() => {
    jest.clearAllMocks()
    ;(global.fetch as jest.Mock).mockReset()
  })

  afterEach(() => {
    jest.clearAllMocks()
  })

  it('should not render when user is not authenticated', async () => {
    ;(global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: false,
      status: 401,
    })

    const { container } = render(
      <AuthProvider>
        <UserMenu />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(container.firstChild).toBeNull()
    })
  })

  it('should render user info when authenticated', async () => {
    ;(global.fetch as jest.Mock).mockResolvedValueOnce({
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
        <UserMenu />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByText('Test User')).toBeInTheDocument()
    })
  })

  it('should display user email if name is not available', async () => {
    ;(global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        sub: 'user123',
        email: 'test@example.com',
        name: null,
      }),
    })

    render(
      <AuthProvider>
        <UserMenu />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByText('test@example.com')).toBeInTheDocument()
    })
  })

  it('should display user sub if neither name nor email available', async () => {
    ;(global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        sub: 'user123',
        email: null,
        name: null,
      }),
    })

    render(
      <AuthProvider>
        <UserMenu />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByText('user123')).toBeInTheDocument()
    })
  })

  it('should call logout when logout button is clicked', async () => {
    ;(global.fetch as jest.Mock)
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          sub: 'user123',
          email: 'test@example.com',
          name: 'Test User',
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
      })

    const user = userEvent.setup()

    render(
      <AuthProvider>
        <UserMenu />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByText('Test User')).toBeInTheDocument()
    })

    // Open dropdown menu
    const userButton = screen.getByText('Test User')
    await user.click(userButton)

    // Wait for dropdown to open and click logout
    await waitFor(() => {
      expect(screen.getByText('Sign out')).toBeInTheDocument()
    })

    const logoutButton = screen.getByText('Sign out')
    await user.click(logoutButton)

    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalledWith(
        'http://localhost:8000/auth/logout',
        expect.objectContaining({
          method: 'POST',
          credentials: 'include',
        })
      )
    })
  })

  it('should show loading state during logout', async () => {
    ;(global.fetch as jest.Mock)
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          sub: 'user123',
          email: 'test@example.com',
          name: 'Test User',
        }),
      })
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            setTimeout(() => resolve({ ok: true, status: 200 }), 100)
          })
      )

    const user = userEvent.setup()

    render(
      <AuthProvider>
        <UserMenu />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByText('Test User')).toBeInTheDocument()
    })

    // Open dropdown
    const userButton = screen.getByText('Test User')
    await user.click(userButton)

    await waitFor(() => {
      expect(screen.getByText('Sign out')).toBeInTheDocument()
    })

    // Click logout
    const logoutButton = screen.getByText('Sign out')
    await user.click(logoutButton)

    // Should show loading state
    await waitFor(() => {
      expect(screen.getByText('Signing out...')).toBeInTheDocument()
    })
  })

  it('should display all user info fields in dropdown', async () => {
    ;(global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        sub: 'user123',
        email: 'test@example.com',
        name: 'Test User',
      }),
    })

    const user = userEvent.setup()

    render(
      <AuthProvider>
        <UserMenu />
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByText('Test User')).toBeInTheDocument()
    })

    // Open dropdown
    const userButton = screen.getByText('Test User')
    await user.click(userButton)

    // Check all user info is displayed in the dropdown
    await waitFor(() => {
      const allTestUserText = screen.getAllByText('Test User')
      expect(allTestUserText.length).toBeGreaterThan(0)
      expect(screen.getByText('test@example.com')).toBeInTheDocument()
    })
  })
})
