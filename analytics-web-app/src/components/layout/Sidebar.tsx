'use client'

import Link from 'next/link'
import { usePathname } from 'next/navigation'
import { LayoutGrid } from 'lucide-react'

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
    matchPaths: ['/processes', '/process', '/process_log', '/process_trace'],
  },
]

export function Sidebar() {
  const pathname = usePathname()

  const isActive = (item: NavItem) => {
    if (item.matchPaths) {
      return item.matchPaths.some((path) => pathname.startsWith(path))
    }
    return pathname === item.href
  }

  return (
    <aside className="hidden sm:flex w-14 bg-[#1a1f26] border-r border-[#2f3540] flex-col py-3">
      <nav className="flex flex-col gap-1">
        {navItems.map((item) => (
          <Link
            key={item.href}
            href={item.href}
            className={`group relative flex items-center justify-center w-10 h-10 mx-2 rounded-md transition-colors ${
              isActive(item)
                ? 'bg-[#22272e] text-blue-500'
                : 'text-gray-400 hover:bg-[#2f3540] hover:text-gray-200'
            }`}
            title={item.label}
          >
            {item.icon}
            <span className="absolute left-16 px-2 py-1 bg-[#2f3540] text-gray-200 text-sm rounded whitespace-nowrap opacity-0 invisible group-hover:opacity-100 group-hover:visible transition-opacity z-50">
              {item.label}
            </span>
          </Link>
        ))}
      </nav>
    </aside>
  )
}
