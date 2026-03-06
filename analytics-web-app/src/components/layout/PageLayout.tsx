import { Suspense } from 'react'
import { Header } from './Header'
import { Sidebar } from './Sidebar'

/**
 * Time range props for controlled time range picker.
 * When provided, the page (controller) owns the time range state.
 */
export interface TimeRangeControlProps {
  /** Raw "from" value (relative like "now-1h" or ISO string) */
  timeRangeFrom: string
  /** Raw "to" value (relative like "now" or ISO string) */
  timeRangeTo: string
  /** Callback when user selects a new time range */
  onTimeRangeChange: (from: string, to: string) => void
}

interface PageLayoutProps {
  children: React.ReactNode
  onRefresh?: () => void
  rightPanel?: React.ReactNode
  /** Time range control props - when provided, page controls time range */
  timeRangeControl?: TimeRangeControlProps
  /** Process ID for pivot button navigation */
  processId?: string
  /** Current auto-refresh interval in ms (0 = off) */
  refreshIntervalMs?: number
  /** Callback to change the auto-refresh interval */
  onRefreshIntervalChange?: (ms: number) => void
  /** Whether the screen is currently executing */
  isExecuting?: boolean
}

function PageLayoutContent({ children, onRefresh, rightPanel, timeRangeControl, processId, refreshIntervalMs, onRefreshIntervalChange, isExecuting }: PageLayoutProps) {
  return (
    <div className="h-screen bg-app-bg text-theme-text-primary flex flex-col">
      <Header onRefresh={onRefresh} timeRangeControl={timeRangeControl} processId={processId} refreshIntervalMs={refreshIntervalMs} onRefreshIntervalChange={onRefreshIntervalChange} isExecuting={isExecuting} />
      <div className="flex flex-1 min-h-0">
        <Sidebar />
        <main className="flex-1 overflow-auto flex flex-col">{children}</main>
        {rightPanel}
      </div>
    </div>
  )
}

export function PageLayout({ children, onRefresh, rightPanel, timeRangeControl, processId, refreshIntervalMs, onRefreshIntervalChange, isExecuting }: PageLayoutProps) {
  return (
    <Suspense
      fallback={
        <div className="min-h-screen bg-app-bg flex items-center justify-center">
          <div className="animate-spin rounded-full h-8 w-8 border-2 border-accent-link border-t-transparent" />
        </div>
      }
    >
      <PageLayoutContent onRefresh={onRefresh} rightPanel={rightPanel} timeRangeControl={timeRangeControl} processId={processId} refreshIntervalMs={refreshIntervalMs} onRefreshIntervalChange={onRefreshIntervalChange} isExecuting={isExecuting}>
        {children}
      </PageLayoutContent>
    </Suspense>
  )
}
