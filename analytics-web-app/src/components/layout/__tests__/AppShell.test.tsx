/**
 * Regression test for issue #1439 (sidebar collapses on screen search).
 *
 * Renders the *real* route tree from `router.tsx` — not a standalone
 * `AppShell` with dummy routes — so it exercises the actual router
 * restructuring: `Sidebar` should mount once at the `AppShell` layout-route
 * level and survive navigation between two different pages, instead of
 * being torn down and rebuilt by each page's own `PageLayout` (the bug's
 * root cause). Lazy page modules and `Sidebar` are mocked; `Sidebar`'s mount
 * effect is tracked via a module-level counter rather than asserting on
 * flyout CSS.
 *
 * Also covers `AppShell`'s auth/admin gate directly: a mis-written gate
 * (e.g. `status !== 'unauthenticated'`) would let `Sidebar`'s unconditional
 * mount effect (`Sidebar.tsx:104-109`) fire `loadFolders()`/`listScreens()`
 * during loading/unauthenticated/admin-denied states, which these cases
 * would catch.
 */
import { useEffect } from 'react'
import { render, screen, fireEvent } from '@testing-library/react'
import { Link, MemoryRouter } from 'react-router'
import { AppRouter } from '@/router'

// The global mock in test-setup.ts stubs out useLocation/useSearchParams,
// which this test needs to be real (AppShell reads useLocation for its
// admin-route check, and Sidebar reads useLocation/useSearchParams too).
vi.mock('react-router', async (importOriginal) => {
  const actual = await importOriginal<typeof import('react-router')>()
  return { ...actual }
})

const authState = vi.hoisted(() => ({
  status: 'authenticated' as 'loading' | 'authenticated' | 'unauthenticated' | 'error',
  user: { sub: 'u1', is_admin: true } as { sub: string; is_admin?: boolean } | null,
}))
vi.mock('@/lib/auth', () => ({
  useAuth: () => authState,
}))

const sidebarMounts = vi.hoisted(() => ({ count: 0 }))
vi.mock('@/components/layout/Sidebar', () => ({
  Sidebar: () => {
    // Effect (not the render body) so re-renders of an already-mounted
    // Sidebar don't inflate the count — only an actual mount should.
    useEffect(() => {
      sidebarMounts.count += 1
    }, [])
    return <div data-testid="sidebar-mock" />
  },
}))

// Lazy page modules: replaced with lightweight stubs so the test exercises
// router.tsx's structure without pulling in each page's real dependencies.
vi.mock('@/routes/LoginPage', () => ({ default: () => <div>login-page</div> }))
vi.mock('@/routes/ProcessesPage', () => ({
  default: () => (
    <div>
      <span>processes-page</span>
      <Link to="/screens">go to screens</Link>
    </div>
  ),
}))
vi.mock('@/routes/ProcessPage', () => ({ default: () => <div>process-page</div> }))
vi.mock('@/routes/ProcessLogPage', () => ({ default: () => <div>process-log-page</div> }))
vi.mock('@/routes/ProcessMetricsPage', () => ({ default: () => <div>process-metrics-page</div> }))
vi.mock('@/routes/PerformanceAnalysisPage', () => ({ default: () => <div>performance-analysis-page</div> }))
vi.mock('@/routes/ScreensPage', () => ({ default: () => <div>screens-page</div> }))
vi.mock('@/routes/ScreenPage', () => ({ default: () => <div>screen-page</div> }))
vi.mock('@/routes/AdminPage', () => ({ default: () => <div>admin-page</div> }))
vi.mock('@/routes/DataSourcesPage', () => ({ default: () => <div>data-sources-page</div> }))
vi.mock('@/routes/ExportScreensPage', () => ({ default: () => <div>export-screens-page</div> }))
vi.mock('@/routes/ImportScreensPage', () => ({ default: () => <div>import-screens-page</div> }))
vi.mock('@/routes/MapsPage', () => ({ default: () => <div>maps-page</div> }))
vi.mock('@/routes/NotFoundPage', () => ({ default: () => <div>not-found-page</div> }))
// `/admin/ingestion-keys` is no longer fully admin-gated (#1544): its real component mounts a
// self-service panel whose effect calls `fetchMyAudiences`/`listIngestionApiKeys` against
// `fetch`, which this file never stubs. Stubbed here so the sidebar-gate cases below exercise
// only `AppShell`'s own gate.
vi.mock('@/routes/IngestionApiKeysPage', () => ({
  default: () => <div>ingestion-api-keys-page</div>,
}))

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AppRouter />
    </MemoryRouter>
  )
}

describe('AppShell (via router.tsx)', () => {
  beforeEach(() => {
    sidebarMounts.count = 0
    authState.status = 'authenticated'
    authState.user = { sub: 'u1', is_admin: true }
  })

  it('mounts Sidebar once and keeps it mounted across a route navigation', async () => {
    renderAt('/processes')

    await screen.findByText('processes-page')
    expect(sidebarMounts.count).toBe(1)

    fireEvent.click(screen.getByText('go to screens'))

    await screen.findByText('screens-page')
    expect(sidebarMounts.count).toBe(1)
  })

  it('does not mount Sidebar while auth status is loading', async () => {
    authState.status = 'loading'

    renderAt('/processes')

    await screen.findByText('processes-page')
    expect(sidebarMounts.count).toBe(0)
  })

  it('does not mount Sidebar when unauthenticated', async () => {
    authState.status = 'unauthenticated'
    authState.user = null

    renderAt('/processes')

    await screen.findByText('processes-page')
    expect(sidebarMounts.count).toBe(0)
  })

  it('does not mount Sidebar on an admin route for a non-admin user', async () => {
    authState.status = 'authenticated'
    authState.user = { sub: 'u1', is_admin: false }

    renderAt('/admin/maps')

    await screen.findByText('maps-page')
    expect(sidebarMounts.count).toBe(0)
  })

  it('mounts Sidebar on /admin for a non-admin user (viewable by everyone, role-filtered content)', async () => {
    authState.status = 'authenticated'
    authState.user = { sub: 'u1', is_admin: false }

    renderAt('/admin')

    await screen.findByText('admin-page')
    expect(sidebarMounts.count).toBe(1)
  })

  it('mounts Sidebar on /admin/ingestion-keys for a non-admin user (mint-only content)', async () => {
    authState.status = 'authenticated'
    authState.user = { sub: 'u1', is_admin: false }

    renderAt('/admin/ingestion-keys')

    await screen.findByText('ingestion-api-keys-page')
    expect(sidebarMounts.count).toBe(1)
  })
})
