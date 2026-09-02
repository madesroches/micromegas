import { Suspense } from 'react'
import { Outlet, useLocation } from 'react-router'
import { useAuth } from '@/lib/auth'
import { PageLoader } from '@/router'
import { Sidebar } from './Sidebar'

// `/admin` and `/admin/ingestion-keys` are viewable by every authenticated user (role-filtered
// content), so they're deliberately excluded here — only the routes still fully gated by
// `<AuthGuard requireAdmin>` need Sidebar hidden from a non-admin.
const ADMIN_ONLY_PATHS = [
  '/admin/data-sources',
  '/admin/export-screens',
  '/admin/import-screens',
  '/admin/maps',
  '/admin/analytics-keys',
  '/admin/query-deny-list',
  '/admin/groups',
]

export function AppShell() {
  const { status, user } = useAuth()
  const { pathname } = useLocation()
  // Mirrors each admin-only page's own `<AuthGuard requireAdmin>` check — an admin-only route
  // added here without a matching path in ADMIN_ONLY_PATHS would mount Sidebar even though
  // AuthGuard would go on to block the page.
  const isAdminOnlyRoute = ADMIN_ONLY_PATHS.some((p) => pathname.startsWith(p))
  const showSidebar = status === 'authenticated' && (!isAdminOnlyRoute || user?.is_admin)

  return (
    <div className="h-screen bg-app-bg">
      {showSidebar && (
        <div className="fixed top-16 left-0 bottom-0 z-40 hidden sm:flex">
          <Sidebar />
        </div>
      )}
      <Suspense fallback={<PageLoader />}>
        <Outlet />
      </Suspense>
    </div>
  )
}
