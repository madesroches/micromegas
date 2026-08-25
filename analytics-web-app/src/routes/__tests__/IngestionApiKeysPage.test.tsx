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

// Page size for the pagination tests. Small on purpose: a "full page" case
// has to return exactly `pageSize` rows for the Next button to appear, and
// rendering the real default (the server's 500-row max) in jsdom is
// noticeably slower — using a small page size here keeps this file's test
// time roughly 6x faster.
const TEST_PAGE_SIZE = 3

function renderPage(pageSize?: number) {
  return render(
    <MemoryRouter>
      <IngestionApiKeysPage pageSize={pageSize} />
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

  it('lists at the server max limit by default', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve([]),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    // No `pageSize` prop — the route renders the page this way, and it must
    // ask for the server's max rather than letting `limit` fall back to the
    // server's lower `DEFAULT_LIMIT` of 100, which silently truncates.
    renderPage()

    await waitFor(() => expect(fetchMock).toHaveBeenCalled())
    expect(fetchMock.mock.calls[0][0]).toContain(`limit=${MAX_INGESTION_API_KEYS_LIST_LIMIT}`)
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

  it('shows an Audience column, rendering — for a key with none', async () => {
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
            audience: 'team-alpha',
          },
          {
            key_id: 'key-2',
            name: 'no-audience-key',
            created_at: '2026-01-01T00:00:00Z',
            created_by: 'alice@example.com',
            last_used_at: null,
            revoked_at: null,
            revoked_by: null,
          },
        ]),
    } as unknown as Response) as unknown as typeof fetch

    renderPage()

    await waitFor(() => expect(screen.getByText('Audience')).toBeInTheDocument())
    expect(screen.getByText('team-alpha')).toBeInTheDocument()
    // '—' also appears in the (null) Last Used column, so there must be at least
    // two occurrences once the no-audience row's Audience cell renders one too.
    expect(screen.getAllByText('—').length).toBeGreaterThanOrEqual(2)
  })

  it('mints a key with an explicit audience', async () => {
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
            audience: 'team-alpha',
          }),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve([]),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage()

    await waitFor(() =>
      expect(screen.getByText(/No ingestion API keys yet/i)).toBeInTheDocument()
    )

    fireEvent.click(screen.getAllByRole('button', { name: /Mint Key/i })[0])
    const nameInput = await screen.findByPlaceholderText(/game-client-42/i)
    fireEvent.change(nameInput, { target: { value: 'new-key' } })
    const audienceInput = await screen.findByPlaceholderText('team-alpha')
    fireEvent.change(audienceInput, { target: { value: 'team-alpha' } })

    const mintButton = screen.getByRole('button', { name: 'Mint' })
    await act(async () => {
      fireEvent.click(mintButton)
    })

    await waitFor(() => {
      const postCall = fetchMock.mock.calls.find((c) => c[1]?.method === 'POST')
      expect(postCall).toBeDefined()
      expect(JSON.parse(postCall![1].body)).toEqual({
        name: 'new-key',
        audience: 'team-alpha',
      })
    })
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
            audience: 'public',
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
              audience: 'public',
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
      expect(JSON.parse(postCall![1].body)).toEqual({ name: 'new-key', audience: 'public' })
    })

    await waitFor(() => expect(screen.getByText('mmk_test_cleartext')).toBeInTheDocument())
    expect(screen.getByText(/won't be shown again/i)).toBeInTheDocument()
  })

  it('shows a Next button when the list returns exactly one full page', async () => {
    global.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve(makeKeys(TEST_PAGE_SIZE)),
    } as unknown as Response) as unknown as typeof fetch

    renderPage(TEST_PAGE_SIZE)

    await waitFor(() => expect(screen.getByText('key-0')).toBeInTheDocument())
    expect(screen.getByRole('button', { name: 'Next' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Previous' })).not.toBeInTheDocument()
  })

  it('shows no Next button when the list returns fewer than a full page', async () => {
    global.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve(makeKeys(TEST_PAGE_SIZE - 1)),
    } as unknown as Response) as unknown as typeof fetch

    renderPage(TEST_PAGE_SIZE)

    await waitFor(() => expect(screen.getByText('key-0')).toBeInTheDocument())
    expect(screen.queryByRole('button', { name: 'Next' })).not.toBeInTheDocument()
  })

  it('clicking Next re-fetches with the next offset and reveals a Previous button', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve(makeKeys(TEST_PAGE_SIZE)),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve(makeKeys(1)),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage(TEST_PAGE_SIZE)

    const nextButton = await screen.findByRole('button', { name: 'Next' })
    fireEvent.click(nextButton)

    await waitFor(() => {
      const [url] = fetchMock.mock.calls[1]
      expect(url).toContain(`offset=${TEST_PAGE_SIZE}`)
    })

    expect(await screen.findByRole('button', { name: 'Previous' })).toBeInTheDocument()
  })

  it('minting while on a later page resets to page 1 so the new key is visible', async () => {
    const fetchMock = vi
      .fn()
      // Initial load: page 1, full page (so Next is shown).
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve(makeKeys(TEST_PAGE_SIZE)),
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

    renderPage(TEST_PAGE_SIZE)

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
        json: () => Promise.resolve({ revoked_at: '2026-01-02T00:00:00Z' }),
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
