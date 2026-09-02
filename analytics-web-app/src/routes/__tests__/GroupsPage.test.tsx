/**
 * Page-level tests for `GroupsPage`: the group list, selecting a group to view/add/remove
 * members, and the cycle-conflict error surfaced from `POST .../members`. Modeled on
 * `AudienceAccessPage.test.tsx`'s `global.fetch` dispatcher harness.
 */
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import GroupsPage from '../GroupsPage'

vi.mock('@/lib/auth', () => ({
  useAuth: () => ({
    status: 'authenticated',
    user: { sub: 'admin', email: 'admin@example.com', is_admin: true },
    error: null,
  }),
}))

vi.mock('@/hooks/usePageTitle', () => ({ usePageTitle: () => undefined }))

vi.mock('@/components/layout', () => ({
  PageLayout: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))

interface RawGroup {
  name: string
  description: string | null
  member_count: number
  created_at: string
  created_by: string
}

interface RawMember {
  group_name: string
  member: string
  created_at: string
  created_by: string
}

function jsonResponse(status: number, body: unknown) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
  } as unknown as Response
}

function installFetchMock(opts: {
  groups: RawGroup[]
  membersByGroup: Record<string, RawMember[]>
  onAddMember?: (name: string, body: unknown) => { status: number; body: unknown }
  onRemoveMember?: (name: string, member: string) => { status: number; body?: unknown }
}) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input)
    const method = init?.method ?? 'GET'

    const membersMatch = url.match(/\/api\/groups\/([^/?]+)\/members(\?.*)?$/)
    if (membersMatch) {
      const name = decodeURIComponent(membersMatch[1])
      if (method === 'GET') {
        return jsonResponse(200, opts.membersByGroup[name] ?? [])
      }
      if (method === 'POST') {
        const body: unknown = init?.body ? JSON.parse(init.body as string) : {}
        const result = opts.onAddMember?.(name, body) ?? {
          status: 201,
          body: {
            group_name: name,
            member: (body as { member: string }).member,
            created_at: '2026-08-14T00:00:00Z',
            created_by: 'admin@example.com',
          },
        }
        return jsonResponse(result.status, result.body)
      }
      if (method === 'DELETE') {
        const query = new URL(url, 'http://localhost').searchParams
        const member = query.get('member') ?? ''
        const result = opts.onRemoveMember?.(name, member) ?? { status: 204 }
        return jsonResponse(result.status, result.body)
      }
    }

    if (url.endsWith('/api/groups') && method === 'GET') {
      return jsonResponse(200, opts.groups)
    }

    return jsonResponse(404, { code: 'NOT_FOUND', message: 'unhandled in test' })
  })
  global.fetch = fetchMock as unknown as typeof fetch
  return fetchMock
}

function renderPage() {
  return render(
    <MemoryRouter>
      <GroupsPage />
    </MemoryRouter>
  )
}

describe('GroupsPage', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('lists groups with member counts', async () => {
    installFetchMock({
      groups: [
        {
          name: 'admins',
          description: 'Deployment administrators',
          member_count: 1,
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'default',
        },
        {
          name: 'eng',
          description: null,
          member_count: 2,
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
      membersByGroup: {},
    })

    renderPage()

    await screen.findByText('admins')
    expect(screen.getByText('eng')).toBeInTheDocument()
  })

  it('selecting a group loads and shows its members', async () => {
    installFetchMock({
      groups: [
        {
          name: 'eng',
          description: null,
          member_count: 1,
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
      membersByGroup: {
        eng: [
          {
            group_name: 'eng',
            member: 'user:alice@example.com',
            created_at: '2026-08-14T00:00:00Z',
            created_by: 'admin@example.com',
          },
        ],
      },
    })

    renderPage()

    fireEvent.click(await screen.findByText('eng'))
    await screen.findByText('user:alice@example.com')
  })

  it('adding a member posts to .../members and refreshes the list', async () => {
    const membersByGroup: Record<string, RawMember[]> = { eng: [] }
    const fetchMock = installFetchMock({
      groups: [
        {
          name: 'eng',
          description: null,
          member_count: 0,
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
      membersByGroup,
      onAddMember: (name, body) => {
        const member = (body as { member: string }).member
        membersByGroup[name] = [
          ...(membersByGroup[name] ?? []),
          { group_name: name, member, created_at: '2026-08-14T00:00:00Z', created_by: 'admin@example.com' },
        ]
        return {
          status: 201,
          body: { group_name: name, member, created_at: '2026-08-14T00:00:00Z', created_by: 'admin@example.com' },
        }
      },
    })

    renderPage()

    fireEvent.click(await screen.findByText('eng'))
    fireEvent.click(await screen.findByText('Add member'))

    const emailInput = await screen.findByPlaceholderText('alice@example.com')
    fireEvent.change(emailInput, { target: { value: 'bob@example.com' } })
    const dialog = screen.getByRole('dialog')
    fireEvent.click(within(dialog).getByText('Add member', { selector: 'button' }))

    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/groups/eng/members',
        expect.objectContaining({ method: 'POST' })
      )
    )
    await screen.findByText('user:bob@example.com')
  })

  it('a cycle-conflict response surfaces the 409 error in the dialog', async () => {
    installFetchMock({
      groups: [
        {
          name: 'a',
          description: null,
          member_count: 0,
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
        {
          name: 'b',
          description: null,
          member_count: 0,
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
      membersByGroup: {},
      onAddMember: () => ({
        status: 409,
        body: { code: 'CONFLICT', message: 'would create a cycle' },
      }),
    })

    renderPage()

    fireEvent.click(await screen.findByText('a'))
    fireEvent.click(await screen.findByText('Add member'))
    fireEvent.click(screen.getByRole('button', { name: 'group' }))

    const select = await screen.findByRole('combobox')
    fireEvent.change(select, { target: { value: 'b' } })
    const dialog = screen.getByRole('dialog')
    fireEvent.click(within(dialog).getByText('Add member', { selector: 'button' }))

    await screen.findByText('Adding this member would create a cycle.')
  })

  it('removing a member calls DELETE with the member query param', async () => {
    const fetchMock = installFetchMock({
      groups: [
        {
          name: 'eng',
          description: null,
          member_count: 1,
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
      membersByGroup: {
        eng: [
          {
            group_name: 'eng',
            member: 'user:alice@example.com',
            created_at: '2026-08-14T00:00:00Z',
            created_by: 'admin@example.com',
          },
        ],
      },
    })

    renderPage()

    fireEvent.click(await screen.findByText('eng'))
    const removeButton = await screen.findByLabelText(
      'Remove user:alice@example.com from eng'
    )
    fireEvent.click(removeButton)
    fireEvent.click(await screen.findByText('Remove', { selector: 'button' }))

    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/groups/eng/members?member=user%3Aalice%40example.com',
        expect.objectContaining({ method: 'DELETE' })
      )
    )
  })
})
