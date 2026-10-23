/**
 * Page-level tests for `SettingsPage`: the source pill, each Edit dialog's submitted JSON per
 * kind, and the re-fetch after a write. Modeled on `GroupsPage.test.tsx`'s `global.fetch`
 * dispatcher harness.
 */
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import SettingsPage from '../SettingsPage'

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

function jsonResponse(status: number, body: unknown) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
  } as unknown as Response
}

function entries(stored: Record<string, unknown> = {}) {
  const base = (key: string, kind: string, def: unknown, env_var: string) => ({
    key,
    kind,
    env_var,
    default: def,
    description: `${key} description`,
    value: key in stored ? stored[key] : null,
    updated_at: key in stored ? '2026-10-21T09:14:00Z' : null,
    updated_by: key in stored ? 'admin@example.com' : null,
  })
  return [
    base('public_view_sets', 'name_list', [], 'MICROMEGAS_PUBLIC_VIEW_SETS'),
    base('self_service_mint', 'bool', false, 'MICROMEGAS_SELF_SERVICE_MINT'),
    base(
      'self_service_max_keys_per_caller',
      'count',
      100,
      'MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER'
    ),
  ]
}

function installFetchMock(initial: Record<string, unknown> = {}) {
  let stored = { ...initial }
  const writes: { method: string; url: string; body: unknown }[] = []
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input)
    const method = init?.method ?? 'GET'
    if (method === 'GET') return jsonResponse(200, entries(stored))
    const key = decodeURIComponent(url.split('/').pop() ?? '')
    const body: unknown = init?.body ? JSON.parse(init.body as string) : undefined
    writes.push({ method, url, body })
    if (method === 'PUT') {
      stored = { ...stored, [key]: (body as { value: unknown }).value }
      return jsonResponse(200, entries(stored).find((e) => e.key === key))
    }
    const { [key]: _removed, ...rest } = stored
    void _removed
    stored = rest
    return jsonResponse(204, undefined)
  })
  global.fetch = fetchMock as unknown as typeof fetch
  return { fetchMock, writes }
}

function renderPage() {
  return render(
    <MemoryRouter>
      <SettingsPage />
    </MemoryRouter>
  )
}

describe('SettingsPage', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('shows Database for a stored value and Env / default otherwise', async () => {
    installFetchMock({ self_service_mint: true })
    renderPage()

    await screen.findByText('self_service_mint')
    expect(screen.getAllByText('Database')).toHaveLength(1)
    expect(screen.getAllByText('Env / default')).toHaveLength(2)
    expect(screen.getByLabelText('Reset self_service_mint')).toBeInTheDocument()
    expect(screen.queryByLabelText('Reset public_view_sets')).not.toBeInTheDocument()
  })

  it('submits a bool, re-fetching afterwards', async () => {
    const { fetchMock, writes } = installFetchMock()
    renderPage()

    fireEvent.click(await screen.findByLabelText('Edit self_service_mint'))
    fireEvent.click(screen.getByLabelText('Enabled'))
    fireEvent.click(screen.getByText('Save'))

    await waitFor(() => expect(writes).toHaveLength(1))
    expect(writes[0].method).toBe('PUT')
    expect(writes[0].body).toEqual({ value: true })
    await waitFor(() => expect(screen.getByText('Database')).toBeInTheDocument())
    const gets = fetchMock.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'GET')
    expect(gets.length).toBeGreaterThanOrEqual(2)
  })

  it('notes the env fallback in the dialog only for an unstored entry', async () => {
    installFetchMock({ self_service_mint: true })
    renderPage()

    fireEvent.click(await screen.findByLabelText('Edit self_service_max_keys_per_caller'))
    expect(screen.getByText(/Pre-filled with the built-in default/)).toBeInTheDocument()
    expect(
      screen.getAllByText('MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER').length
    ).toBeGreaterThan(0)
    fireEvent.click(screen.getByLabelText('Close'))

    fireEvent.click(await screen.findByLabelText('Edit self_service_mint'))
    expect(screen.queryByText(/Pre-filled with the built-in default/)).not.toBeInTheDocument()
  })

  it('submits a count as a number', async () => {
    const { writes } = installFetchMock()
    renderPage()

    fireEvent.click(await screen.findByLabelText('Edit self_service_max_keys_per_caller'))
    fireEvent.change(screen.getByLabelText('Value'), { target: { value: '20' } })
    fireEvent.click(screen.getByText('Save'))

    await waitFor(() => expect(writes).toHaveLength(1))
    expect(writes[0].body).toEqual({ value: 20 })
  })

  it('submits a name list built from chips', async () => {
    const { writes } = installFetchMock()
    renderPage()

    fireEvent.click(await screen.findByLabelText('Edit public_view_sets'))
    const input = screen.getByLabelText('View sets')
    fireEvent.change(input, { target: { value: 'log_stats' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    fireEvent.change(input, { target: { value: 'images' } })
    fireEvent.click(screen.getByText('Save'))

    await waitFor(() => expect(writes).toHaveLength(1))
    expect(writes[0].body).toEqual({ value: ['log_stats', 'images'] })
  })

  it('resets a stored value after confirmation and re-fetches', async () => {
    const { writes } = installFetchMock({ self_service_mint: true })
    renderPage()

    fireEvent.click(await screen.findByLabelText('Reset self_service_mint'))
    fireEvent.click(screen.getByRole('button', { name: 'Reset' }))

    await waitFor(() => expect(writes).toHaveLength(1))
    expect(writes[0].method).toBe('DELETE')
    await waitFor(() => expect(screen.queryByText('Database')).not.toBeInTheDocument())
  })
})
