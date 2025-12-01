'use client'

import { Suspense } from 'react'
import { Header } from './Header'
import { Sidebar } from './Sidebar'

interface PageLayoutProps {
  children: React.ReactNode
  onRefresh?: () => void
}

function PageLayoutContent({ children, onRefresh }: PageLayoutProps) {
  return (
    <div className="min-h-screen bg-[#0f1419] text-gray-200">
      <Header onRefresh={onRefresh} />
      <div className="flex h-[calc(100vh-57px)]">
        <Sidebar />
        <main className="flex-1 overflow-auto">{children}</main>
      </div>
    </div>
  )
}

export function PageLayout({ children, onRefresh }: PageLayoutProps) {
  return (
    <Suspense
      fallback={
        <div className="min-h-screen bg-[#0f1419] flex items-center justify-center">
          <div className="animate-spin rounded-full h-8 w-8 border-2 border-blue-500 border-t-transparent" />
        </div>
      }
    >
      <PageLayoutContent onRefresh={onRefresh}>{children}</PageLayoutContent>
    </Suspense>
  )
}
