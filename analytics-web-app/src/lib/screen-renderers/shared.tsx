import { ReactNode } from 'react'
import { ErrorBanner } from '@/components/ErrorBanner'

/**
 * Standard loading state for screen renderers.
 * Optional - renderers can use their own loading UI.
 */
export function LoadingState({ message = 'Loading...' }: { message?: string }) {
  return (
    <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
      <div className="flex items-center gap-3">
        <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
        <span className="text-theme-text-secondary">{message}</span>
      </div>
    </div>
  )
}

/**
 * Standard empty state for screen renderers.
 * Optional - renderers can use their own empty UI.
 */
export function EmptyState({ message = 'No data available.' }: { message?: string }) {
  return (
    <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
      <span className="text-theme-text-muted">{message}</span>
    </div>
  )
}

export interface RendererLayoutProps {
  /** Query error message */
  error: string | null
  /** Whether error is retryable */
  isRetryable?: boolean
  /** Retry handler */
  onRetry?: () => void
  /** SQL panel (QueryEditor) */
  sqlPanel: ReactNode
  /** Main content */
  children: ReactNode
  /** Optional controls above content (filters, etc.) */
  controls?: ReactNode
}

/**
 * Standard two-column layout for screen renderers.
 * Optional - renderers can use their own layout.
 *
 * Layout: [content + controls | sql panel]
 */
export function RendererLayout({
  error,
  isRetryable,
  onRetry,
  sqlPanel,
  children,
  controls,
}: RendererLayoutProps) {
  return (
    <div className="flex h-full">
      <div className="flex-1 flex flex-col p-6 min-w-0">
        {controls}
        {error && (
          <ErrorBanner
            title="Query execution failed"
            message={error}
            onRetry={isRetryable ? onRetry : undefined}
          />
        )}
        {children}
      </div>
      {sqlPanel}
    </div>
  )
}
