import { ReactNode } from 'react'
import { Save } from 'lucide-react'
import { Button } from '@/components/ui/button'
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

export interface SaveFooterProps {
  /** Handler for saving existing screen (null if new screen) */
  onSave: (() => Promise<void>) | (() => Promise<unknown>) | null
  /** Handler for "Save As" dialog */
  onSaveAs: () => void
  /** Whether save is in progress */
  isSaving: boolean
  /** Whether there are unsaved changes */
  hasUnsavedChanges: boolean
  /** Error message from save operation */
  saveError: string | null
}

/**
 * Standard save buttons footer for QueryEditor.
 * Optional - renderers can build their own footer.
 */
export function SaveFooter({
  onSave,
  onSaveAs,
  isSaving,
  hasUnsavedChanges,
  saveError,
}: SaveFooterProps) {
  return (
    <>
      <div className="border-t border-theme-border p-3 flex gap-2">
        {onSave && (
          <Button
            variant="default"
            size="sm"
            onClick={onSave}
            disabled={isSaving || !hasUnsavedChanges}
            className="gap-1"
          >
            <Save className="w-4 h-4" />
            {isSaving ? 'Saving...' : 'Save'}
          </Button>
        )}
        <Button variant="outline" size="sm" onClick={onSaveAs} className="gap-1">
          <Save className="w-4 h-4" />
          Save As
        </Button>
      </div>
      {saveError && (
        <div className="px-3 pb-3">
          <p className="text-xs text-accent-error">{saveError}</p>
        </div>
      )}
    </>
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
