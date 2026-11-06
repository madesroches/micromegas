import { Suspense } from 'react'
import { Outlet, useLocation } from 'react-router'
import { useAuth } from '@/lib/auth'
import { PageLoader } from '@/router'
import { ADMIN_COLOR, AdminModeProvider, isAdminRoute, useAdminMode } from '@/lib/admin-mode'
import { Sidebar } from './Sidebar'

function AdminFrame() {
  const { active } = useAdminMode()
  if (!active) return null
  return (
    <div
      data-testid="admin-mode-frame"
      className="pointer-events-none fixed inset-0 z-50 border-2"
      style={{ borderColor: ADMIN_COLOR }}
    />
  )
}

export function AppShell() {
  const { status, user } = useAuth()
  const { pathname } = useLocation()
  // Mirrors each admin-only page's own `<AuthGuard requireAdmin>` check — an admin-only route
  // added here without a matching path in ADMIN_ONLY_PATHS would mount Sidebar even though
  // AuthGuard would go on to block the page.
  const isAdminOnlyRoute = isAdminRoute(pathname)
  const showSidebar = status === 'authenticated' && (!isAdminOnlyRoute || user?.is_admin)

  return (
    <AdminModeProvider pathname={pathname}>
    <div className="h-screen bg-app-bg">
      <AdminFrame />
      {showSidebar && (
        <div className="fixed top-16 left-0 bottom-0 z-40 hidden sm:flex">
          <Sidebar />
        </div>
      )}
      <Suspense fallback={<PageLoader />}>
        <Outlet />
      </Suspense>
    </div>
    </AdminModeProvider>
  )
}
