import { cleanup, render, screen, fireEvent } from '@testing-library/react'
import { useState } from 'react'
import { MemoryRouter } from 'react-router'
import { AdminModeProvider, isAdminRoute, useRegisterAdminModeToggle } from '@/lib/admin-mode'
import { Header } from '../Header'
import { RefreshIntervalPicker } from '../RefreshIntervalPicker'

vi.mock('react-router', async (importOriginal) => {
  const actual = await importOriginal<typeof import('react-router')>()
  return { ...actual }
})

const authState = vi.hoisted(() => ({
  status: 'authenticated',
  user: { sub: 'u1', name: 'Ada Admin', is_admin: true } as { sub: string; name: string; is_admin: boolean },
  logout: vi.fn(),
}))
vi.mock('@/lib/auth', () => ({ useAuth: () => authState }))

function NotebookPage({ enabled = true }: { enabled?: boolean }) {
  const [active, setActive] = useState(false)
  useRegisterAdminModeToggle(enabled, active, setActive)
  return <div data-testid="active">{String(active)}</div>
}

function renderHeader(path: string, page?: React.ReactNode, refresh = true) {
  return render(
    <MemoryRouter>
      <AdminModeProvider pathname={path} isAdmin={authState.user.is_admin}>
        <Header
          onRefresh={refresh ? vi.fn() : undefined}
          onRefreshIntervalChange={refresh ? vi.fn() : undefined}
          refreshIntervalMs={30_000}
          timeRangeControl={undefined}
        />
        {page}
      </AdminModeProvider>
    </MemoryRouter>
  )
}

function openMenu() {
  // The menu trigger is the only button holding the avatar initials.
  fireEvent.click(screen.getByText('AA'))
}

describe('isAdminRoute', () => {
  it('matches admin-only routes by exact prefix', () => {
    expect(isAdminRoute('/admin/query-deny-list')).toBe(true)
    expect(isAdminRoute('/admin/maps/foo')).toBe(true)
    expect(isAdminRoute('/admin/maps-foo')).toBe(false)
    expect(isAdminRoute('/admin')).toBe(false)
    expect(isAdminRoute('/admin/ingestion-keys')).toBe(false)
  })
})

describe('Header admin mode', () => {
  afterEach(cleanup)

  beforeEach(() => {
    authState.user = { sub: 'u1', name: 'Ada Admin', is_admin: true }
  })

  it('shows the stripe and pill on admin pages, not on /admin or ingestion keys', () => {
    const { unmount } = renderHeader('/admin/query-deny-list')
    expect(screen.getByTestId('admin-mode-stripe')).toBeInTheDocument()
    expect(screen.getByText('ADMIN')).toBeInTheDocument()
    unmount()
    for (const path of ['/admin', '/admin/ingestion-keys']) {
      const r = renderHeader(path)
      expect(screen.queryByTestId('admin-mode-stripe')).not.toBeInTheDocument()
      r.unmount()
    }
  })

  it('offers Enable admin mode to an admin on a notebook and toggles it', () => {
    renderHeader('/screen/nb', <NotebookPage />)
    expect(screen.queryByTestId('admin-mode-stripe')).not.toBeInTheDocument()
    openMenu()
    fireEvent.click(screen.getByText('Enable admin mode'))
    expect(screen.getByTestId('active')).toHaveTextContent('true')
    expect(screen.getByText('ADMIN MODE')).toBeInTheDocument()
    expect(screen.getByTestId('admin-mode-stripe')).toBeInTheDocument()

    openMenu()
    expect(screen.getByText('Disable admin mode')).toBeInTheDocument()
    fireEvent.click(screen.getByText('Turn off'))
    expect(screen.getByTestId('active')).toHaveTextContent('false')
    expect(screen.queryByText('ADMIN MODE')).not.toBeInTheDocument()
  })

  it('does not offer the toggle to a non-admin or on pages without a notebook', () => {
    authState.user = { sub: 'u2', name: 'Ada Admin', is_admin: false }
    const r = renderHeader('/screen/nb', <NotebookPage />)
    openMenu()
    expect(screen.queryByText('Enable admin mode')).not.toBeInTheDocument()
    r.unmount()

    authState.user = { sub: 'u1', name: 'Ada Admin', is_admin: true }
    renderHeader('/screen/other', <NotebookPage enabled={false} />)
    openMenu()
    expect(screen.queryByText('Enable admin mode')).not.toBeInTheDocument()
  })

  it('pauses the refresh picker while the toggle is on', () => {
    renderHeader('/screen/nb', <NotebookPage />)
    expect(screen.getByTitle('Auto-refreshing every 30s')).toBeInTheDocument()
    openMenu()
    fireEvent.click(screen.getByText('Enable admin mode'))
    expect(screen.getByText('30s · paused')).toBeInTheDocument()
    expect(screen.getByTitle('Auto-refresh interval')).toBeDisabled()
    expect(screen.getByTitle('Paused while admin mode is on')).toBeInTheDocument()
  })
})

describe('RefreshIntervalPicker paused', () => {
  it('disables the dropdown and keeps Refresh active', () => {
    const onRefresh = vi.fn()
    render(
      <RefreshIntervalPicker intervalMs={5_000} onIntervalChange={vi.fn()} onRefresh={onRefresh} paused />
    )
    expect(screen.getByTitle('Auto-refresh interval')).toBeDisabled()
    fireEvent.click(screen.getByTitle('Paused while admin mode is on'))
    expect(onRefresh).toHaveBeenCalled()
  })
})
