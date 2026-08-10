/**
 * Light-touch tests for `IngestionApiKeysPage` — list render, mint form
 * submit + one-time-key banner, and revoke confirm flow. Modeled on
 * `MapsPage.test.tsx`'s render-and-probe-fetch-calls style.
 */
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import IngestionApiKeysPage from '../IngestionApiKeysPage'
import { MAX_INGESTION_API_KEYS_LIST_LIMIT } from '@/lib/ingestion-api-keys-api'

function makeKeys(count: number) {
  return Array.from({ length: count }, (_, i) => ({
    key_id: `key-${i}`,
    name: `key-${i}`,
    created_at: '2026-01-01T00:00:00Z',
    created_by: 'alice@example.com',
    last_used_at: null,
    revoked_at: null,
    revoked_by: null,
  }))
}

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
      <IngestionApiKeysPage />
    </MemoryRouter>
  )
}

describe('IngestionApiKeysPage', () => {
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
      expect(screen.getByText(/No ingestion API keys yet/i)).toBeInTheDocument()
    )
  })

  it('lists existing keys from the list response', async () => {
    global.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: () =>
        Promise.resolve([
          {
            key_id: 'key-1',
            name: 'game-client-42',
            created_at: '2026-01-01T00:00:00Z',
            created_by: 'alice@example.com',
            last_used_at: null,
            revoked_at: null,
            revoked_by: null,
          },
        ]),
    } as unknown as Response) as unknown as typeof fetch

    renderPage()

    await waitFor(() => expect(screen.getByText('game-client-42')).toBeInTheDocument())
    expect(screen.getByText('Active')).toBeInTheDocument()
  })

  it('mints a key via the proxy and shows the one-time-key banner', async () => {
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
      expect(screen.getByText(/No ingestion API keys yet/i)).toBeInTheDocument()
    )

    fireEvent.click(screen.getAllByRole('button', { name: /Mint Key/i })[0])
    const nameInput = await screen.findByPlaceholderText(/game-client-42/i)
    fireEvent.change(nameInput, { target: { value: 'new-key' } })

    const mintButton = screen.getByRole('button', { name: 'Mint' })
    await act(async () => {
      fireEvent.click(mintButton)
    })

    await waitFor(() => {
      const postCall = fetchMock.mock.calls.find((c) => c[1]?.method === 'POST')
      expect(postCall).toBeDefined()
      expect(postCall![0]).toBe('/mmlocal/api/ingestion-api-keys')
      expect(JSON.parse(postCall![1].body)).toEqual({ name: 'new-key' })
    })

    await waitFor(() => expect(screen.getByText('mmk_test_cleartext')).toBeInTheDocument())
    expect(screen.getByText(/won't be shown again/i)).toBeInTheDocument()
  })

  it('shows a Next button when the list returns exactly the max limit', async () => {
    global.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve(makeKeys(MAX_INGESTION_API_KEYS_LIST_LIMIT)),
    } as unknown as Response) as unknown as typeof fetch

    renderPage()

    await waitFor(() => expect(screen.getByText('key-0')).toBeInTheDocument())
    expect(screen.getByRole('button', { name: 'Next' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Previous' })).not.toBeInTheDocument()
  })

  it('shows no Next button when the list returns fewer than the max limit', async () => {
    global.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve(makeKeys(1)),
    } as unknown as Response) as unknown as typeof fetch

    renderPage()

    await waitFor(() => expect(screen.getByText('key-0')).toBeInTheDocument())
    expect(screen.queryByRole('button', { name: 'Next' })).not.toBeInTheDocument()
  })

  it('clicking Next re-fetches with the next offset and reveals a Previous button', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve(makeKeys(MAX_INGESTION_API_KEYS_LIST_LIMIT)),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve(makeKeys(1)),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage()

    const nextButton = await screen.findByRole('button', { name: 'Next' })
    fireEvent.click(nextButton)

    await waitFor(() => {
      const [url] = fetchMock.mock.calls[1]
      expect(url).toContain(`offset=${MAX_INGESTION_API_KEYS_LIST_LIMIT}`)
    })

    expect(await screen.findByRole('button', { name: 'Previous' })).toBeInTheDocument()
  })

  it('minting while on a later page resets to page 1 so the new key is visible', async () => {
    const fetchMock = vi
      .fn()
      // Initial load: page 1, full page (so Next is shown).
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve(makeKeys(MAX_INGESTION_API_KEYS_LIST_LIMIT)),
      } as unknown as Response)
      // Load after clicking Next: page 2.
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve(makeKeys(1)),
      } as unknown as Response)
      // Mint call.
      .mockResolvedValueOnce({
        ok: true,
        json: () =>
          Promise.resolve({
            key_id: 'key-new',
            name: 'new-key',
            created_at: '2026-01-01T00:00:00Z',
            key: 'mmk_test_cleartext',
          }),
      } as unknown as Response)
      // Reload after mint: should be page 1 again (offset=0).
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve(makeKeys(1)),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage()

    const nextButton = await screen.findByRole('button', { name: 'Next' })
    fireEvent.click(nextButton)
    await waitFor(() => expect(fetchMock.mock.calls.length).toBe(2))

    fireEvent.click(screen.getAllByRole('button', { name: /Mint Key/i })[0])
    const nameInput = await screen.findByPlaceholderText(/game-client-42/i)
    fireEvent.change(nameInput, { target: { value: 'new-key' } })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Mint' }))
    })

    await waitFor(() => {
      const [url] = fetchMock.mock.calls[3]
      expect(url).toContain('offset=0')
    })
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
              name: 'game-client-42',
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
        json: () =>
          Promise.resolve({ revoked_at: '2026-01-02T00:00:00Z', effective_within_seconds: 60 }),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve([]),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage()

    await waitFor(() => expect(screen.getByText('game-client-42')).toBeInTheDocument())

    fireEvent.click(screen.getByRole('button', { name: /Revoke game-client-42/i }))
    const confirmButton = await screen.findByRole('button', { name: 'Revoke' })
    await act(async () => {
      fireEvent.click(confirmButton)
    })

    await waitFor(() => {
      const deleteCall = fetchMock.mock.calls.find((c) => c[1]?.method === 'DELETE')
      expect(deleteCall).toBeDefined()
      expect(deleteCall![0]).toBe('/mmlocal/api/ingestion-api-keys/key-1')
    })
  })
})
