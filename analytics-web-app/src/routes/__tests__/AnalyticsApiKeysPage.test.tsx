/**
 * Light-touch tests for `AnalyticsApiKeysPage` — list render, mint form
 * submit + one-time-key banner, and revoke confirm flow. Modeled on
 * `MapsPage.test.tsx`'s render-and-probe-fetch-calls style.
 */
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import AnalyticsApiKeysPage from '../AnalyticsApiKeysPage'

// Force useAuth to report an admin user so AuthGuard renders the page.
vi.mock('@/lib/auth', () => ({
  useAuth: () => ({
    status: 'authenticated',
    user: { sub: 'admin', is_admin: true },
    error: null,
  }),
}))

// Pin basePath to a known value so the URL assertions are stable.
vi.mock('@/lib/config', () => ({
  getConfig: () => ({ basePath: '/mmlocal' }),
  appLink: (path: string) => `/mmlocal${path}`,
}))

vi.mock('@/hooks/usePageTitle', () => ({ usePageTitle: () => undefined }))

vi.mock('@/components/layout', () => ({
  PageLayout: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))

function renderPage() {
  return render(
    <MemoryRouter>
      <AnalyticsApiKeysPage />
    </MemoryRouter>
  )
}

describe('AnalyticsApiKeysPage', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('renders the empty state when there are no keys', async () => {
    global.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve([]),
    } as unknown as Response) as unknown as typeof fetch

    renderPage()

    await waitFor(() =>
      expect(screen.getByText(/No analytics API keys yet/i)).toBeInTheDocument()
    )
  })

  it('lists existing keys from the list response', async () => {
    global.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: () =>
        Promise.resolve([
          {
            key_id: 'key-1',
            name: 'grafana-datasource',
            created_at: '2026-01-01T00:00:00Z',
            created_by: 'alice@example.com',
            last_used_at: null,
            revoked_at: null,
            revoked_by: null,
          },
        ]),
    } as unknown as Response) as unknown as typeof fetch

    renderPage()

    await waitFor(() => expect(screen.getByText('grafana-datasource')).toBeInTheDocument())
    expect(screen.getByText('Active')).toBeInTheDocument()
  })

  it('mints a key and shows the one-time-key banner', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve([]),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () =>
          Promise.resolve({
            key_id: 'key-1',
            name: 'new-key',
            created_at: '2026-01-01T00:00:00Z',
            key: 'mmk_test_cleartext',
          }),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () =>
          Promise.resolve([
            {
              key_id: 'key-1',
              name: 'new-key',
              created_at: '2026-01-01T00:00:00Z',
              created_by: 'alice@example.com',
              last_used_at: null,
              revoked_at: null,
              revoked_by: null,
            },
          ]),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage()

    await waitFor(() =>
      expect(screen.getByText(/No analytics API keys yet/i)).toBeInTheDocument()
    )

    fireEvent.click(screen.getAllByRole('button', { name: /Mint Key/i })[0])
    const nameInput = await screen.findByPlaceholderText(/grafana-datasource/i)
    fireEvent.change(nameInput, { target: { value: 'new-key' } })

    const mintButton = screen.getByRole('button', { name: 'Mint' })
    await act(async () => {
      fireEvent.click(mintButton)
    })

    await waitFor(() => {
      const postCall = fetchMock.mock.calls.find((c) => c[1]?.method === 'POST')
      expect(postCall).toBeDefined()
      expect(postCall![0]).toBe('/mmlocal/api/analytics-api-keys')
      expect(JSON.parse(postCall![1].body)).toEqual({ name: 'new-key' })
    })

    await waitFor(() => expect(screen.getByText('mmk_test_cleartext')).toBeInTheDocument())
    expect(screen.getByText(/won't be shown again/i)).toBeInTheDocument()
  })

  it('opens the revoke confirm dialog and DELETEs the right URL on confirm', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        json: () =>
          Promise.resolve([
            {
              key_id: 'key-1',
              name: 'grafana-datasource',
              created_at: '2026-01-01T00:00:00Z',
              created_by: 'alice@example.com',
              last_used_at: null,
              revoked_at: null,
              revoked_by: null,
            },
          ]),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({ revoked_at: '2026-01-02T00:00:00Z' }),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve([]),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage()

    await waitFor(() => expect(screen.getByText('grafana-datasource')).toBeInTheDocument())

    fireEvent.click(screen.getByRole('button', { name: /Revoke grafana-datasource/i }))
    const confirmButton = await screen.findByRole('button', { name: 'Revoke' })
    await act(async () => {
      fireEvent.click(confirmButton)
    })

    await waitFor(() => {
      const deleteCall = fetchMock.mock.calls.find((c) => c[1]?.method === 'DELETE')
      expect(deleteCall).toBeDefined()
      expect(deleteCall![0]).toBe('/mmlocal/api/analytics-api-keys/key-1')
    })
  })
})
