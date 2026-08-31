/**
 * Page-level tests for `AudienceAccessPage` (#1510): grouping, the admin/non-admin gating split,
 * the knob-off narrowing, and the create/delete/mint write flows. Modeled on
 * `QueryDenyListPage.test.tsx`'s harness, minus its data-source/`streamQuery` mocks -- this page
 * has no `DataSourceField` and reads via REST (`global.fetch`), not flight-SQL.
 */
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import AudienceAccessPage from '../AudienceAccessPage'

const authState = vi.hoisted(() => ({
  user: { sub: 'admin', email: 'admin@example.com', is_admin: true },
}))

vi.mock('@/lib/auth', () => ({
  useAuth: () => ({
    status: 'authenticated',
    user: authState.user,
    error: null,
  }),
}))

vi.mock('@/hooks/usePageTitle', () => ({ usePageTitle: () => undefined }))

vi.mock('@/components/layout', () => ({
  PageLayout: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))

interface RawGrant {
  audience: string
  axis: 'read' | 'mint'
  selector: string
  created_at: string
  created_by: string
}

interface MockOptions {
  grants?: RawGrant[]
  myAudiencesStatus?: number
  myAudiences?: {
    is_admin: boolean
    audiences: string[]
    mint_prefix: string | null
    email: string | null
    held_pairs?: string[]
  }
  onCreate?: (body: unknown) => { status: number; body: unknown }
  onDelete?: () => { status: number; body?: unknown }
  onMint?: (body: unknown) => { status: number; body: unknown }
}

function jsonResponse(status: number, body: unknown) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
  } as unknown as Response
}

function installFetchMock(options: MockOptions) {
  const grants = options.grants ?? []
  const myAudiencesStatus = options.myAudiencesStatus ?? 200
  const myAudiences =
    options.myAudiences ?? { is_admin: true, audiences: [], mint_prefix: null, email: null }

  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input)
    const method = init?.method ?? 'GET'

    if (url.includes('/audience-grants/visible')) {
      return jsonResponse(200, grants)
    }
    if (url.includes('/audience-grants/my-audiences')) {
      return myAudiencesStatus === 200
        ? jsonResponse(200, myAudiences)
        : jsonResponse(myAudiencesStatus, {
            code: 'FORBIDDEN',
            message: 'self-service minting is disabled',
          })
    }
    if (url.includes('/audience-grants') && method === 'POST') {
      const body = init?.body ? JSON.parse(init.body as string) : {}
      const result = options.onCreate
        ? options.onCreate(body)
        : {
            status: 201,
            body: { ...body, created_at: '2026-08-14T00:00:00Z', created_by: 'admin@example.com' },
          }
      return jsonResponse(result.status, result.body)
    }
    if (url.includes('/audience-grants') && method === 'DELETE') {
      const result = options.onDelete ? options.onDelete() : { status: 204 }
      return jsonResponse(result.status, result.body)
    }
    if (url.includes('/ingestion-api-keys') && method === 'POST') {
      const body = init?.body ? JSON.parse(init.body as string) : {}
      const result = options.onMint
        ? options.onMint(body)
        : {
            status: 201,
            body: {
              key_id: 'key-1',
              name: body.name,
              created_at: '2026-08-14T00:00:00Z',
              audience: body.audience ?? 'public',
              key: 'mmk_secret',
              claimed: false,
            },
          }
      return jsonResponse(result.status, result.body)
    }
    throw new Error(`unexpected fetch: ${method} ${url}`)
  })
  global.fetch = fetchMock as unknown as typeof fetch
  return fetchMock
}

function renderPage() {
  return render(
    <MemoryRouter>
      <AudienceAccessPage />
    </MemoryRouter>
  )
}

beforeEach(() => {
  authState.user = { sub: 'admin', email: 'admin@example.com', is_admin: true }
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe('AudienceAccessPage — admin', () => {
  it('groups grants by audience and axis, with per-audience counts', async () => {
    installFetchMock({
      grants: [
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'group:eng',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
        {
          audience: 'team-alpha',
          axis: 'mint',
          selector: 'user:alice@example.com',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
    })

    renderPage()

    await waitFor(() => expect(screen.getByText('team-alpha')).toBeInTheDocument())
    expect(screen.getByText('2 grants')).toBeInTheDocument()
    expect(screen.getByText('group:eng')).toBeInTheDocument()
    expect(screen.getByText('user:alice@example.com')).toBeInTheDocument()
    expect(screen.getByText(/2 grants across 1 audiences/)).toBeInTheDocument()
  })

  it('shows the Add grant header button and allows a * selector', async () => {
    const fetchMock = installFetchMock({ grants: [] })
    renderPage()

    await waitFor(() => expect(screen.getByText(/No audience grants yet/)).toBeInTheDocument())

    fireEvent.click(screen.getAllByRole('button', { name: /Add grant/i })[0])
    const audienceInput = await screen.findByPlaceholderText('team-alpha')
    fireEvent.change(audienceInput, { target: { value: 'new-audience' } })

    const heading = screen.getByRole('heading', { name: 'Add audience grant' })
    const dialogRoot = heading.closest('div.relative') as HTMLElement
    // Default selector kind for "Add grant" is Everyone (`*`).
    fireEvent.click(within(dialogRoot).getByRole('button', { name: 'Add grant' }))

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        (c) => String(c[0]).endsWith('/audience-grants') && (c[1] as RequestInit)?.method === 'POST'
      )
      expect(call).toBeDefined()
      const body = JSON.parse((call![1] as RequestInit).body as string)
      expect(body).toEqual({ audience: 'new-audience', axis: 'read', selector: '*' })
    })
  })

  it('the Axis filter hides rows for the other axis without changing the summary line', async () => {
    installFetchMock({
      grants: [
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'group:eng',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
        {
          audience: 'team-alpha',
          axis: 'mint',
          selector: 'user:alice@example.com',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
    })
    renderPage()

    await waitFor(() => expect(screen.getByText('group:eng')).toBeInTheDocument())
    expect(screen.getByText('user:alice@example.com')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'read' }))

    expect(screen.getByText('group:eng')).toBeInTheDocument()
    expect(screen.queryByText('user:alice@example.com')).not.toBeInTheDocument()
    // The summary line is never narrowed by filters.
    expect(screen.getByText(/2 grants across 1 audiences/)).toBeInTheDocument()
  })

  it('Find keeps whole cards, never hiding chips within a surviving card', async () => {
    installFetchMock({
      grants: [
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'group:eng',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
        {
          audience: 'team-beta',
          axis: 'read',
          selector: 'user:bob@example.com',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
    })
    renderPage()

    await waitFor(() => expect(screen.getByText('team-alpha')).toBeInTheDocument())

    fireEvent.change(screen.getByPlaceholderText('Find audience or selector'), {
      target: { value: 'alpha' },
    })

    expect(screen.getByText('team-alpha')).toBeInTheDocument();
    expect(screen.getByText('group:eng')).toBeInTheDocument()
    expect(screen.queryByText('team-beta')).not.toBeInTheDocument()
  })

  it('shows the empty state with an Add action when there are no grants', async () => {
    installFetchMock({ grants: [] })
    renderPage()

    await waitFor(() => expect(screen.getByText(/No audience grants yet/)).toBeInTheDocument())
    expect(screen.getAllByRole('button', { name: /Add grant/i }).length).toBeGreaterThan(0)
  })

  it('does not show the public-readability help line in the Mint dialog for an admin', async () => {
    installFetchMock({ grants: [] })
    renderPage()

    await waitFor(() =>
      expect(screen.getAllByRole('button', { name: /Mint ingestion key/i }).length).toBeGreaterThan(0)
    )
    fireEvent.click(screen.getAllByRole('button', { name: /Mint ingestion key/i })[0])

    const heading = await screen.findByRole('heading', { name: 'Mint ingestion key' })
    const dialogRoot = heading.closest('div.relative') as HTMLElement
    expect(
      within(dialogRoot).queryByText(/is readable by every authenticated user/)
    ).not.toBeInTheDocument()
  })
})

describe('AudienceAccessPage — non-admin', () => {
  beforeEach(() => {
    authState.user = { sub: 'reader', email: 'reader@example.com', is_admin: false }
  })

  it('shows only the rows the server returns, hides the header Add button, and Share offers User/Group only', async () => {
    installFetchMock({
      grants: [
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'user:reader@example.com',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'reader@example.com',
        },
      ],
      myAudiences: {
        is_admin: false,
        audiences: ['team-alpha'],
        mint_prefix: 'reader-',
        email: 'reader@example.com',
        held_pairs: ['team-alpha:read'],
      },
    })
    renderPage()

    await waitFor(() => expect(screen.getByText('team-alpha')).toBeInTheDocument())
    expect(screen.queryByRole('button', { name: /Add grant/i })).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: /Share read access/i }))
    await screen.findByRole('heading', { name: /Share read access/i })
    // No "Everyone" option in the share dialog.
    expect(screen.queryByRole('button', { name: /^everyone$/i })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: /^user$/i })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /^group$/i })).toBeInTheDocument()
  })

  it('hides Share on a pair visible only via a group: row the caller is not confirmed to hold', async () => {
    // Regression test: `/visible` returns every row on a pair the caller can merely *see*
    // (wider than what they *hold*) -- a `group:eng` row here does not, by itself, tell the
    // client whether `reader@example.com` is actually a member of `eng`. Without a held-pairs
    // pair in `myAudiences`, the client must not guess "yes" from the selector prefix alone.
    installFetchMock({
      grants: [
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'group:eng',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
      myAudiences: {
        is_admin: false,
        audiences: [],
        mint_prefix: 'reader-',
        email: 'reader@example.com',
        held_pairs: [],
      },
    })
    renderPage()

    await waitFor(() => expect(screen.getByText('group:eng')).toBeInTheDocument())
    expect(screen.queryByRole('button', { name: /Share read access/i })).not.toBeInTheDocument()
  })

  it('shows Share on a pair the caller holds per held_pairs, even via a group: row', async () => {
    installFetchMock({
      grants: [
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'group:eng',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
      myAudiences: {
        is_admin: false,
        audiences: [],
        mint_prefix: 'reader-',
        email: 'reader@example.com',
        held_pairs: ['team-alpha:read'],
      },
    })
    renderPage()

    await waitFor(() => expect(screen.getByText('group:eng')).toBeInTheDocument())
    expect(screen.getByRole('button', { name: /Share read access/i })).toBeInTheDocument()
  })

  it('shows the chip delete control only on the caller\'s own row', async () => {
    installFetchMock({
      grants: [
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'user:reader@example.com',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'reader@example.com',
        },
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'user:other@example.com',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'other@example.com',
        },
      ],
      myAudiences: {
        is_admin: false,
        audiences: [],
        mint_prefix: 'reader-',
        email: 'reader@example.com',
      },
    })
    renderPage()

    await waitFor(() => expect(screen.getByText('user:reader@example.com')).toBeInTheDocument())
    expect(
      screen.getByRole('button', { name: /Remove my access.*user:reader@example.com/ })
    ).toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: /user:other@example.com/ })
    ).not.toBeInTheDocument()
  })

  it("hides the chip delete control on the caller's own mint row (the claim marker)", async () => {
    installFetchMock({
      grants: [
        {
          audience: 'team-alpha',
          axis: 'mint',
          selector: 'user:reader@example.com',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'reader@example.com',
        },
      ],
      myAudiences: {
        is_admin: false,
        audiences: [],
        mint_prefix: 'reader-',
        email: 'reader@example.com',
      },
    })
    renderPage()

    await waitFor(() => expect(screen.getByText('user:reader@example.com')).toBeInTheDocument())
    // Unlike the read-axis own row above, the server refuses to delete this one -- it's the
    // self-service claim marker `max_claims_per_caller` counts from -- so the button must not
    // render at all rather than render and 403 on click.
    expect(
      screen.queryByRole('button', { name: /user:reader@example.com/ })
    ).not.toBeInTheDocument()
  })

  it('renders a 403 on create inline in the dialog', async () => {
    installFetchMock({
      grants: [
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'user:reader@example.com',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'reader@example.com',
        },
      ],
      myAudiences: {
        is_admin: false,
        audiences: ['team-alpha'],
        mint_prefix: 'reader-',
        email: 'reader@example.com',
        // Client believes it holds this pair (so the Share button renders at all); this test
        // exercises the dialog's inline-403 rendering for the case where the server disagrees
        // (e.g. the hold was revoked between the page load and the share attempt).
        held_pairs: ['team-alpha:read'],
      },
      onCreate: () => ({
        status: 403,
        body: { code: 'FORBIDDEN', message: 'you have no read grant on team-alpha to share' },
      }),
    })
    renderPage()

    await waitFor(() => expect(screen.getByText('team-alpha')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: /Share read access/i }))
    const idInput = await screen.findByPlaceholderText('alice@example.com')
    fireEvent.change(idInput, { target: { value: 'teammate@example.com' } })
    fireEvent.click(screen.getByRole('button', { name: 'Share' }))

    await waitFor(() =>
      expect(screen.getByText(/you have no read grant on team-alpha to share/)).toBeInTheDocument()
    )
  })

  it('mint dialog lists mintable audiences, prefixes a new name, and shows the claimed line', async () => {
    installFetchMock({
      grants: [],
      myAudiences: {
        is_admin: false,
        audiences: ['team-alpha'],
        mint_prefix: 'reader-',
        email: 'reader@example.com',
      },
      onMint: (body) => ({
        status: 201,
        body: {
          key_id: 'key-1',
          name: (body as { name: string }).name,
          created_at: '2026-08-14T00:00:00Z',
          audience: 'reader-myproj',
          key: 'mmk_secret',
          claimed: true,
        },
      }),
    })
    renderPage()

    await waitFor(() =>
      expect(screen.getAllByRole('button', { name: /Mint ingestion key/i }).length).toBeGreaterThan(0)
    )
    fireEvent.click(screen.getAllByRole('button', { name: /Mint ingestion key/i })[0])

    const heading = await screen.findByRole('heading', { name: 'Mint ingestion key' })
    const dialogRoot = heading.closest('div.relative') as HTMLElement

    fireEvent.change(within(dialogRoot).getByPlaceholderText('my-laptop'), {
      target: { value: 'my-key' },
    })
    const select = within(dialogRoot).getByRole('combobox') as HTMLSelectElement
    fireEvent.change(select, { target: { value: '__new__' } })
    fireEvent.change(within(dialogRoot).getByPlaceholderText('myproj'), {
      target: { value: 'myproj' },
    })

    fireEvent.click(within(dialogRoot).getByRole('button', { name: 'Mint' }))

    await waitFor(() => expect(screen.getByText('mmk_secret')).toBeInTheDocument())
    expect(screen.getByText(/You claimed/)).toBeInTheDocument()
    expect(screen.getByText('reader-myproj')).toBeInTheDocument()
  })

  it('shows the public-readability help line under the Mint dialog audience select', async () => {
    installFetchMock({
      grants: [],
      myAudiences: {
        is_admin: false,
        audiences: ['team-alpha'],
        mint_prefix: 'reader-',
        email: 'reader@example.com',
        held_pairs: ['team-alpha:mint'],
      },
    })
    renderPage()

    await waitFor(() =>
      expect(screen.getAllByRole('button', { name: /Mint ingestion key/i }).length).toBeGreaterThan(0)
    )
    fireEvent.click(screen.getAllByRole('button', { name: /Mint ingestion key/i })[0])

    const heading = await screen.findByRole('heading', { name: 'Mint ingestion key' })
    const dialogRoot = heading.closest('div.relative') as HTMLElement
    expect(within(dialogRoot).getByText(/is readable by every authenticated user/)).toBeInTheDocument()
  })
})

describe('AudienceAccessPage — non-admin, self-service disabled', () => {
  beforeEach(() => {
    authState.user = { sub: 'reader', email: 'reader@example.com', is_admin: false }
  })

  it('shows the disabled note, hides Share/Mint, and still renders the list', async () => {
    installFetchMock({
      grants: [
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'user:reader@example.com',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'reader@example.com',
        },
      ],
      myAudiencesStatus: 403,
    })
    renderPage()

    await waitFor(() => expect(screen.getByText('team-alpha')).toBeInTheDocument())
    expect(screen.getByText(/Self-service is disabled on this deployment/)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Mint ingestion key/i })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Share read access/i })).not.toBeInTheDocument()
  })
})

describe('AudienceAccessPage — auth disabled', () => {
  it('renders a single explanatory panel, with no write controls, when /my-audiences 503s AUTH_DISABLED', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url.includes('/audience-grants/visible')) {
        return jsonResponse(200, [])
      }
      if (url.includes('/audience-grants/my-audiences')) {
        return jsonResponse(503, {
          code: 'AUTH_DISABLED',
          message: 'key management is unavailable when auth is disabled',
        })
      }
      throw new Error(`unexpected fetch: ${url}`)
    })
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage()

    await waitFor(() =>
      expect(
        screen.getByText(/Audience grant management is unavailable when authentication is disabled/)
      ).toBeInTheDocument()
    )
    expect(screen.queryByRole('button', { name: /Add grant/i })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Mint ingestion key/i })).not.toBeInTheDocument()
  })

  it('renders the same panel when /visible 503s AUTH_DISABLED', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url.includes('/audience-grants/visible')) {
        return jsonResponse(503, {
          code: 'AUTH_DISABLED',
          message: 'key management is unavailable when auth is disabled',
        })
      }
      if (url.includes('/audience-grants/my-audiences')) {
        return jsonResponse(503, {
          code: 'AUTH_DISABLED',
          message: 'key management is unavailable when auth is disabled',
        })
      }
      throw new Error(`unexpected fetch: ${url}`)
    })
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage()

    await waitFor(() =>
      expect(
        screen.getByText(/Audience grant management is unavailable when authentication is disabled/)
      ).toBeInTheDocument()
    )
  })
})

describe('AudienceAccessPage — errors and reload', () => {
  it('shows an error banner with retry when the list read fails', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url.includes('/audience-grants/visible')) {
        return jsonResponse(500, { code: 'DATABASE_ERROR', message: 'internal database error' })
      }
      if (url.includes('/audience-grants/my-audiences')) {
        return jsonResponse(200, { is_admin: true, audiences: [], mint_prefix: null, email: null })
      }
      throw new Error(`unexpected fetch: ${url}`)
    })
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage()

    await waitFor(() => expect(screen.getByText('internal database error')).toBeInTheDocument())
    expect(fetchMock.mock.calls.filter((c) => String(c[0]).includes('/visible')).length).toBe(1)

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    await waitFor(() =>
      expect(fetchMock.mock.calls.filter((c) => String(c[0]).includes('/visible')).length).toBe(2)
    )
  })

  it('reloads the list from the server after a delete instead of patching locally', async () => {
    let deleted = false
    const fetchMock = installFetchMock({
      grants: [
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'group:eng',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
      onDelete: () => {
        deleted = true
        return { status: 204 }
      },
    })
    renderPage()

    await waitFor(() => expect(screen.getByText('group:eng')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: /group:eng/ }))
    const confirmButtons = screen.getAllByRole('button', { name: 'Delete' })
    fireEvent.click(confirmButtons[confirmButtons.length - 1])

    await waitFor(() => expect(deleted).toBe(true))
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.filter((c) => String(c[0]).includes('/visible')).length
      ).toBeGreaterThanOrEqual(2)
    )
  })
})
