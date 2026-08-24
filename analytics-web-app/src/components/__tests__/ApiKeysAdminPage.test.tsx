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
    maxListLimit: 20,
    ErrorClass: TestApiKeyError,
    listKeys: vi.fn().mockResolvedValue([]),
    mintKey: vi.fn(),
    revokeKey: vi.fn(),
    ...overrides,
  }
}

function renderPage(config: ApiKeysAdminPageConfig) {
  return render(
    <MemoryRouter>
      <ApiKeysAdminPage config={config} />
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
