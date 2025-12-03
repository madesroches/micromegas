'use client'

import { Suspense } from 'react'
import { Header } from './Header'
import { Sidebar } from './Sidebar'

interface PageLayoutProps {
  children: React.ReactNode
  onRefresh?: () => void
  rightPanel?: React.ReactNode
}

function PageLayoutContent({ children, onRefresh, rightPanel }: PageLayoutProps) {
  return (
    <div className="min-h-screen bg-app-bg text-theme-text-primary">
      <Header onRefresh={onRefresh} />
      <div className="flex h-[calc(100vh-57px)]">
        <Sidebar />
        <main className="flex-1 overflow-auto">{children}</main>
        {rightPanel}
      </div>
    </div>
  )
}

export function PageLayout({ children, onRefresh, rightPanel }: PageLayoutProps) {
  return (
    <Suspense
      fallback={
        <div className="min-h-screen bg-app-bg flex items-center justify-center">
          <div className="animate-spin rounded-full h-8 w-8 border-2 border-accent-link border-t-transparent" />
        </div>
      }
    >
      <PageLayoutContent onRefresh={onRefresh} rightPanel={rightPanel}>
        {children}
      </PageLayoutContent>
    </Suspense>
  )
}
