/**
 * `AdminPage` (#1544): the hub is viewable by every authenticated user, with its card grid
 * filtered by role — an admin sees all eight cards, a non-admin only the two with a real
 * non-admin capability (Ingestion API Keys, Audience Access).
 */
import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import AdminPage from '../AdminPage'

const authState = vi.hoisted(() => ({
  user: { sub: 'admin', is_admin: true } as { sub: string; is_admin?: boolean } | null,
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

function renderPage() {
  return render(
    <MemoryRouter>
      <AdminPage />
    </MemoryRouter>
  )
}

const adminOnlyTitles = [
  'Data Sources',
  'Export Screens',
  'Import Screens',
  'Maps',
  'Analytics API Keys',
  'Query Deny List',
]

describe('AdminPage — admin', () => {
  beforeEach(() => {
    authState.user = { sub: 'admin', is_admin: true }
  })

  it('shows all eight cards and the admin subtitle', async () => {
    renderPage()

    await screen.findByText('Ingestion API Keys')
    for (const title of [...adminOnlyTitles, 'Ingestion API Keys', 'Audience Access']) {
      expect(screen.getByText(title)).toBeInTheDocument()
    }
    expect(
      screen.getByText('System administration and data management tools.')
    ).toBeInTheDocument()
    expect(
      screen.getByText(/Mint, list, and revoke write credentials/)
    ).toBeInTheDocument()
  })
})

describe('AdminPage — non-admin', () => {
  beforeEach(() => {
    authState.user = { sub: 'reader', is_admin: false }
  })

  it('shows only Ingestion API Keys and Audience Access, and the non-admin subtitle', async () => {
    renderPage()

    await screen.findByText('Ingestion API Keys')
    expect(screen.getByText('Audience Access')).toBeInTheDocument()
    for (const title of adminOnlyTitles) {
      expect(screen.queryByText(title)).not.toBeInTheDocument()
    }
    expect(screen.getByText('Tools you have access to.')).toBeInTheDocument()
  })

  it('shows the role-aware, self-service-only description for Ingestion API Keys', async () => {
    renderPage()

    await screen.findByText('Ingestion API Keys')
    expect(
      screen.getByText('Mint your own write credentials for telemetry ingestion clients.')
    ).toBeInTheDocument()
    expect(screen.queryByText(/Mint, list, and revoke write credentials/)).not.toBeInTheDocument()
  })
})
