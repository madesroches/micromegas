import { Suspense } from 'react'
import { Outlet, useLocation } from 'react-router'
import { useAuth } from '@/lib/auth'
import { Sidebar } from './Sidebar'

function OutletFallback() {
  return <div className="h-screen flex items-center justify-center text-theme-text-secondary">Loading...</div>
}

export function AppShell() {
  const { status, user } = useAuth()
  const { pathname } = useLocation()
  const isAdminRoute = pathname.startsWith('/admin')
  const showSidebar = status === 'authenticated' && (!isAdminRoute || user?.is_admin)

  return (
    <div className="h-screen bg-app-bg">
      {showSidebar && (
        <div className="fixed top-16 left-0 bottom-0 z-40 hidden sm:flex">
          <Sidebar />
        </div>
      )}
      <Suspense fallback={<OutletFallback />}>
        <Outlet />
      </Suspense>
    </div>
  )
}
