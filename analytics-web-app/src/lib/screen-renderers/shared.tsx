import { ReactNode } from 'react'
import { X } from 'lucide-react'
import { ErrorBanner } from '@/components/ErrorBanner'
import { CELL_TYPE_OPTIONS } from './cell-registry'
import type { CellType } from './notebook-types'

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

interface AddCellModalProps {
  isOpen: boolean
  onClose: () => void
  onAdd: (type: CellType) => void
  title?: string
  excludeTypes?: CellType[]
}

export function AddCellModal({ isOpen, onClose, onAdd, title = 'Add Cell', excludeTypes }: AddCellModalProps) {
  if (!isOpen) return null

  const options = excludeTypes ? CELL_TYPE_OPTIONS.filter((o) => !excludeTypes.includes(o.type)) : CELL_TYPE_OPTIONS

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={onClose} />
      <div className="relative w-full max-w-sm bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
          <h2 className="text-lg font-medium text-theme-text-primary">{title}</h2>
          <button
            onClick={onClose}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>
        <div className="p-2">
          {options.map((option) => (
            <button
              key={option.type}
              onClick={() => onAdd(option.type)}
              className="w-full flex items-center gap-3 p-3 rounded-lg hover:bg-app-card transition-colors text-left"
            >
              <div className="w-10 h-10 bg-app-card rounded-lg flex items-center justify-center text-lg font-semibold text-theme-text-secondary">
                {option.icon}
              </div>
              <div>
                <div className="font-medium text-theme-text-primary">{option.name}</div>
                <div className="text-xs text-theme-text-muted">{option.description}</div>
              </div>
            </button>
          ))}
        </div>
      </div>
    </div>
  )
}
