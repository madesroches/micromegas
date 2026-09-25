/**
 * Covers the mint-dialog validation added for ingestion-key audiences
 * (#1372 follow-up): `openMintForm` pre-filling `mintAudience` with
 * "public" only when `config.showAudience` is set, and the Mint button's
 * `disabled` expression gating on a blank/whitespace audience in that case.
 */
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { ApiKeysAdminPage, ApiKeysAdminPageConfig } from '../ApiKeysAdminPage'

// Force useAuth to report an admin user so AuthGuard renders the page.
vi.mock('@/lib/auth', () => ({
  useAuth: () => ({
    status: 'authenticated',
    user: { sub: 'admin', is_admin: true },
    error: null,
  }),
}))

vi.mock('@/components/layout', () => ({
  PageLayout: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))

class TestApiKeyError extends Error {}

function makeConfig(overrides: Partial<ApiKeysAdminPageConfig>): ApiKeysAdminPageConfig {
  return {
    title: 'Test Keys',
    subtitle: 'subtitle',
    mintDialogTitle: 'Mint Test Key',
    namePlaceholder: 'e.g. test-client',
    emptyStateText: 'No test keys yet.',
    loadErrorMessage: 'Failed to load test keys',
    revokeConfirmMessage: (name) => `Revoke "${name}"?`,
    ErrorClass: TestApiKeyError,
    listKeys: vi.fn().mockResolvedValue([]),
    mintKey: vi.fn(),
    revokeKey: vi.fn(),
    setAllowlist: vi.fn(),
    ...overrides,
  }
}

function renderPage(config: ApiKeysAdminPageConfig) {
  return render(
    <MemoryRouter>
      <ApiKeysAdminPage config={config} pageSize={20} />
    </MemoryRouter>
  )
}

async function openMintDialog(config: ApiKeysAdminPageConfig) {
  await waitFor(() => expect(screen.getByText(config.emptyStateText)).toBeInTheDocument())
  fireEvent.click(screen.getAllByRole('button', { name: /Mint Key/i })[0])
  return screen.findByRole('button', { name: 'Mint' })
}

describe('ApiKeysAdminPage mint dialog', () => {
  it('pre-fills audience with "public" and keeps Mint disabled until name and audience are both set (showAudience: true)', async () => {
    const config = makeConfig({ showAudience: true })
    renderPage(config)

    const mintButton = await openMintDialog(config)

    const audienceInput = screen.getByPlaceholderText('team-alpha')
    expect(audienceInput).toHaveValue('public')

    // Name still blank -> disabled regardless of the pre-filled audience.
    expect(mintButton).toBeDisabled()

    const nameInput = screen.getByPlaceholderText(config.namePlaceholder)
    fireEvent.change(nameInput, { target: { value: 'new-key' } })
    // Name set, audience pre-filled with "public" -> enabled.
    expect(mintButton).not.toBeDisabled()

    fireEvent.change(audienceInput, { target: { value: '' } })
    expect(mintButton).toBeDisabled()

    fireEvent.change(audienceInput, { target: { value: '   ' } })
    expect(mintButton).toBeDisabled()

    fireEvent.change(audienceInput, { target: { value: 'team-alpha' } })
    expect(mintButton).not.toBeDisabled()
  })

  it('states that minting requires a mint grant and that a fresh audience claims itself (showAudience: true)', async () => {
    const config = makeConfig({ showAudience: true })
    renderPage(config)
    await openMintDialog(config)

    expect(screen.getByText(/Minting requires a mint grant on this audience/)).toBeInTheDocument()
    expect(screen.getByText(/Naming a brand-new audience claims it and grants you read \+ mint on it\./)).toBeInTheDocument()
  })

  it('has no audience field and enables Mint from just a name (showAudience: false)', async () => {
    const config = makeConfig({ showAudience: false })
    renderPage(config)

    const mintButton = await openMintDialog(config)

    expect(screen.queryByPlaceholderText('team-alpha')).not.toBeInTheDocument()
    expect(screen.queryByText('Audience', { selector: 'label' })).not.toBeInTheDocument()

    expect(mintButton).toBeDisabled()

    const nameInput = screen.getByPlaceholderText(config.namePlaceholder)
    fireEvent.change(nameInput, { target: { value: 'new-key' } })
    expect(mintButton).not.toBeDisabled()
  })
})

/**
 * IP allowlist column, edit dialog, and mint-form field (#1611).
 */
describe('ApiKeysAdminPage IP allowlist', () => {
  function keyRow(overrides: Partial<Record<string, unknown>> = {}) {
    return {
      key_id: 'key-1',
      name: 'my-key',
      created_at: '2026-01-01T00:00:00Z',
      created_by: 'alice@example.com',
      last_used_at: null,
      revoked_at: null,
      revoked_by: null,
      allowed_cidrs: [],
      ...overrides,
    }
  }

  it('shows "Unrestricted" for an empty allowlist and one line per entry otherwise', async () => {
    const config = makeConfig({
      listKeys: vi.fn().mockResolvedValue([
        keyRow({ key_id: 'key-1', name: 'unrestricted-key' }),
        keyRow({
          key_id: 'key-2',
          name: 'restricted-key',
          allowed_cidrs: ['10.0.0.0/8', '203.0.113.7'],
        }),
      ]),
    })
    renderPage(config)

    await waitFor(() => expect(screen.getByText('unrestricted-key')).toBeInTheDocument())
    expect(screen.getByText('Unrestricted')).toBeInTheDocument()
    expect(screen.getByText('10.0.0.0/8')).toBeInTheDocument()
    expect(screen.getByText('203.0.113.7')).toBeInTheDocument()
  })

  it('hides the edit button on a revoked row', async () => {
    const config = makeConfig({
      listKeys: vi.fn().mockResolvedValue([
        keyRow({ key_id: 'key-1', name: 'active-key' }),
        keyRow({ key_id: 'key-2', name: 'revoked-key', revoked_at: '2026-01-02T00:00:00Z' }),
      ]),
    })
    renderPage(config)

    await waitFor(() => expect(screen.getByText('active-key')).toBeInTheDocument())
    expect(
      screen.getByRole('button', { name: 'Edit IP allowlist for active-key' })
    ).toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: 'Edit IP allowlist for revoked-key' })
    ).not.toBeInTheDocument()
  })

  it('opens the edit dialog prefilled with the entries joined by newlines, saves, closes, and reloads', async () => {
    const setAllowlist = vi.fn().mockResolvedValue({ allowed_cidrs: ['10.0.0.0/8'] })
    const listKeys = vi
      .fn()
      .mockResolvedValueOnce([
        keyRow({ allowed_cidrs: ['10.0.0.0/8', '203.0.113.7'] }),
      ])
      .mockResolvedValueOnce([keyRow({ allowed_cidrs: ['10.0.0.0/8'] })])
    const config = makeConfig({ listKeys, setAllowlist })
    renderPage(config)

    await waitFor(() => expect(screen.getByText('my-key')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'Edit IP allowlist for my-key' }))

    const heading = await screen.findByRole('heading', { name: 'IP allowlist — my-key' })
    const textarea = screen.getByPlaceholderText(/203\.0\.113\.0\/24/) as HTMLTextAreaElement
    expect(textarea.value).toBe('10.0.0.0/8\n203.0.113.7')

    fireEvent.change(textarea, { target: { value: '10.0.0.0/8' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await waitFor(() => expect(setAllowlist).toHaveBeenCalledWith('key-1', ['10.0.0.0/8']))
    await waitFor(() => expect(heading).not.toBeInTheDocument())
    await waitFor(() => expect(listKeys).toHaveBeenCalledTimes(2))
  })

  it('clearing the textarea saves []', async () => {
    const setAllowlist = vi.fn().mockResolvedValue({ allowed_cidrs: [] })
    const config = makeConfig({
      listKeys: vi.fn().mockResolvedValue([keyRow({ allowed_cidrs: ['10.0.0.0/8'] })]),
      setAllowlist,
    })
    renderPage(config)

    await waitFor(() => expect(screen.getByText('my-key')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'Edit IP allowlist for my-key' }))

    const textarea = await screen.findByPlaceholderText(/203\.0\.113\.0\/24/)
    fireEvent.change(textarea, { target: { value: '' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await waitFor(() => expect(setAllowlist).toHaveBeenCalledWith('key-1', []))
  })

  it('keeps the dialog open and shows the error message when setAllowlist is rejected', async () => {
    const setAllowlist = vi.fn().mockRejectedValue(new TestApiKeyError('invalid allowed_cidrs: invalid CIDR or IP address: "foo"'))
    const config = makeConfig({
      listKeys: vi.fn().mockResolvedValue([keyRow()]),
      setAllowlist,
    })
    renderPage(config)

    await waitFor(() => expect(screen.getByText('my-key')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'Edit IP allowlist for my-key' }))

    const textarea = await screen.findByPlaceholderText(/203\.0\.113\.0\/24/)
    fireEvent.change(textarea, { target: { value: 'foo' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await waitFor(() =>
      expect(
        screen.getByText(/invalid allowed_cidrs: invalid CIDR or IP address/)
      ).toBeInTheDocument()
    )
    expect(screen.getByRole('heading', { name: 'IP allowlist — my-key' })).toBeInTheDocument()
  })

  it('the inline mint form passes allowed_cidrs when filled and undefined when blank', async () => {
    const mintKey = vi.fn().mockResolvedValue({ key_id: 'key-new', name: 'new-key', created_at: 't', key: 'k' })
    const config = makeConfig({ mintKey })
    renderPage(config)

    const mintButton = await openMintDialog(config)
    fireEvent.change(screen.getByPlaceholderText(config.namePlaceholder), {
      target: { value: 'new-key' },
    })

    // Blank allowlist -> undefined.
    fireEvent.click(mintButton)
    await waitFor(() =>
      expect(mintKey).toHaveBeenCalledWith('new-key', { audience: undefined, allowed_cidrs: undefined })
    )

    mintKey.mockClear()
    fireEvent.click(screen.getAllByRole('button', { name: /Mint Key/i })[0])
    fireEvent.change(screen.getByPlaceholderText(config.namePlaceholder), {
      target: { value: 'new-key-2' },
    })
    fireEvent.change(screen.getByPlaceholderText(/203\.0\.113\.0\/24/), {
      target: { value: '10.0.0.0/8, 203.0.113.7' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Mint' }))

    await waitFor(() =>
      expect(mintKey).toHaveBeenCalledWith('new-key-2', {
        audience: undefined,
        allowed_cidrs: ['10.0.0.0/8', '203.0.113.7'],
      })
    )
  })
})
