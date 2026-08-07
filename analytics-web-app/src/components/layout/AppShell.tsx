import { Suspense } from 'react'
import { Outlet, useLocation } from 'react-router'
import { useAuth } from '@/lib/auth'
import { PageLoader } from '@/router'
import { Sidebar } from './Sidebar'

export function AppShell() {
  const { status, user } = useAuth()
  const { pathname } = useLocation()
  // `/admin` prefix stands in for each admin page's own `<AuthGuard requireAdmin>`
  // check — an admin-only route added outside `/admin` would mount Sidebar here
  // even though AuthGuard would go on to block the page.
  const isAdminRoute = pathname.startsWith('/admin')
  const showSidebar = status === 'authenticated' && (!isAdminRoute || user?.is_admin)

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
