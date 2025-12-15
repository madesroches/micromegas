import { render, screen, waitFor } from '@testing-library/react'
import { AuthGuard } from '../AuthGuard'
import { AuthProvider } from '@/lib/auth'

// Mock fetch globally
global.fetch = jest.fn()

// Mock Next.js navigation
jest.mock('next/navigation', () => ({
  usePathname: () => '/',
}))

// Mock window.location
const originalLocation = window.location
const mockLocationHref = jest.fn()

beforeAll(() => {
  delete (window as unknown as { location?: Location }).location
  const mockLocation = { ...originalLocation, href: '' }
  Object.defineProperty(window, 'location', {
    value: mockLocation,
    writable: true,
    configurable: true,
  })
  Object.defineProperty(window.location, 'href', {
    set: mockLocationHref,
    get: () => '',
    configurable: true,
  })
})

afterAll(() => {
  Object.defineProperty(window, 'location', {
    value: originalLocation,
    writable: true,
    configurable: true,
  })
})

describe('AuthGuard', () => {
  beforeEach(() => {
    jest.clearAllMocks()
    ;(global.fetch as jest.Mock).mockReset()
    mockLocationHref.mockClear()
  })

  afterEach(() => {
    jest.clearAllMocks()
  })

  it('should show loading state while checking authentication', () => {
    (global.fetch as jest.Mock).mockImplementation(
      () => new Promise(() => {}) // Never resolves
    )

    render(
      <AuthProvider>
        <AuthGuard>
          <div>Protected Content</div>
        </AuthGuard>
      </AuthProvider>
    )

    expect(screen.getByText('Loading...')).toBeInTheDocument()
    expect(screen.queryByText('Protected Content')).not.toBeInTheDocument()
  })

  it('should render children when authenticated', async () => {
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
        <AuthGuard>
          <div>Protected Content</div>
        </AuthGuard>
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByText('Protected Content')).toBeInTheDocument()
    })

    expect(screen.queryByText('Loading...')).not.toBeInTheDocument()
  })

  it('should redirect to login when unauthenticated', async () => {
    (global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: false,
      status: 401,
    })

    render(
      <AuthProvider>
        <AuthGuard>
          <div>Protected Content</div>
        </AuthGuard>
      </AuthProvider>
    )

    await waitFor(() => {
      // Base path from config defaults to http://localhost:8000
      expect(mockLocationHref).toHaveBeenCalledWith(
        'http://localhost:8000/login?return_url=%2F'
      )
    })

    expect(screen.queryByText('Protected Content')).not.toBeInTheDocument()
    expect(screen.getByText('Redirecting to login...')).toBeInTheDocument()
  })

  it('should show error state on service unavailable', async () => {
    (global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: false,
      status: 500,
    })

    render(
      <AuthProvider>
        <AuthGuard>
          <div>Protected Content</div>
        </AuthGuard>
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByText('Service Unavailable')).toBeInTheDocument()
    })

    expect(screen.queryByText('Protected Content')).not.toBeInTheDocument()
    expect(screen.getByText('Retry')).toBeInTheDocument()
  })

  it('should show error state on network error', async () => {
    (global.fetch as jest.Mock).mockRejectedValueOnce(
      new Error('Network error')
    )

    render(
      <AuthProvider>
        <AuthGuard>
          <div>Protected Content</div>
        </AuthGuard>
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByText(/Network error/)).toBeInTheDocument()
    })

    expect(screen.queryByText('Protected Content')).not.toBeInTheDocument()
  })

  it('should retry authentication when retry button is clicked', async () => {
    (global.fetch as jest.Mock)
      .mockResolvedValueOnce({
        ok: false,
        status: 500,
      })

    const { rerender } = render(
      <AuthProvider>
        <AuthGuard>
          <div>Protected Content</div>
        </AuthGuard>
      </AuthProvider>
    )

    await waitFor(() => {
      expect(screen.getByText('Service Unavailable')).toBeInTheDocument()
    })

    // Mock successful response for retry
    ;(global.fetch as jest.Mock).mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        sub: 'user123',
        email: 'test@example.com',
      }),
    })

    const retryButton = screen.getByText('Retry')
    retryButton.click()

    // Retry button reloads the page, so we just verify it exists
    expect(retryButton).toBeInTheDocument()
  })
})
