import { useLocation } from 'react-router-dom'
import { AppLink } from '@/components/AppLink'
import { useAuth } from '@/lib/auth'
import { LayoutGrid, Layers, Wrench } from 'lucide-react'

interface NavItem {
  href: string
  icon: React.ReactNode
  label: string
  matchPaths?: string[]
}

const navItems: NavItem[] = [
  {
    href: '/processes',
    icon: <LayoutGrid className="w-5 h-5" />,
    label: 'Processes',
    matchPaths: ['/processes', '/process', '/process_log', '/process_metrics', '/performance_analysis'],
  },
  {
    href: '/screens',
    icon: <Layers className="w-5 h-5" />,
    label: 'Screens',
    matchPaths: ['/screens', '/screen'],
  },
]

export function Sidebar() {
  const location = useLocation()
  const { user } = useAuth()
  const pathname = location.pathname

  const isActive = (item: NavItem) => {
    if (item.matchPaths) {
      return item.matchPaths.some((path) => pathname.startsWith(path))
    }
    return pathname === item.href
  }

  const adminItem: NavItem = {
    href: '/admin',
    icon: <Wrench className="w-5 h-5" />,
    label: 'Admin',
    matchPaths: ['/admin'],
  }

  return (
    <aside className="hidden sm:flex w-14 bg-app-sidebar border-r border-theme-border flex-col py-3">
      <nav className="flex flex-col gap-1">
        {navItems.map((item) => (
          <AppLink
            key={item.href}
            href={item.href}
            className={`group relative flex items-center justify-center w-10 h-10 mx-2 rounded-md transition-colors ${
              isActive(item)
                ? 'bg-app-card text-accent-link'
                : 'text-theme-text-secondary hover:bg-theme-border hover:text-theme-text-primary'
            }`}
            title={item.label}
          >
            {item.icon}
            <span className="absolute left-16 px-2 py-1 bg-theme-border text-theme-text-primary text-sm rounded whitespace-nowrap opacity-0 invisible group-hover:opacity-100 group-hover:visible transition-opacity z-50">
              {item.label}
            </span>
          </AppLink>
        ))}
      </nav>
      {user?.is_admin && (
        <div className="mt-auto">
          <div className="h-px bg-theme-border mx-2 mb-1" />
          <nav className="flex flex-col gap-1">
            <AppLink
              href={adminItem.href}
              className={`group relative flex items-center justify-center w-10 h-10 mx-2 rounded-md transition-colors ${
                isActive(adminItem)
                  ? 'bg-app-card text-accent-link'
                  : 'text-theme-text-secondary hover:bg-theme-border hover:text-theme-text-primary'
              }`}
              title={adminItem.label}
            >
              {adminItem.icon}
              <span className="absolute left-16 px-2 py-1 bg-theme-border text-theme-text-primary text-sm rounded whitespace-nowrap opacity-0 invisible group-hover:opacity-100 group-hover:visible transition-opacity z-50">
                {adminItem.label}
              </span>
            </AppLink>
          </nav>
        </div>
      )}
    </aside>
  )
}
