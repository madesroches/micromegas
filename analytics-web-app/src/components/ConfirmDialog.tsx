import { useCallback, useEffect } from 'react'
import { X, AlertTriangle } from 'lucide-react'
import { Button } from '@/components/ui/button'

interface ConfirmDialogProps {
  isOpen: boolean
  onClose: () => void
  onConfirm: () => void
  title: string
  message: string
  confirmLabel?: string
  cancelLabel?: string
  isLoading?: boolean
  variant?: 'danger' | 'default'
}

export function ConfirmDialog({
  isOpen,
  onClose,
  onConfirm,
  title,
  message,
  confirmLabel = 'Confirm',
  cancelLabel = 'Cancel',
  isLoading = false,
  variant = 'default',
}: ConfirmDialogProps) {
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !isLoading) {
        onClose()
      }
    },
    [onClose, isLoading]
  )

  useEffect(() => {
    if (isOpen) {
      document.addEventListener('keydown', handleKeyDown)
      return () => document.removeEventListener('keydown', handleKeyDown)
    }
  }, [isOpen, handleKeyDown])

  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      {/* Backdrop */}
      <div
        className="absolute inset-0 bg-black/50"
        onClick={isLoading ? undefined : onClose}
      />

      {/* Dialog */}
      <div className="relative w-full max-w-sm bg-app-panel border border-theme-border rounded-lg shadow-xl">
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
          <div className="flex items-center gap-2">
            {variant === 'danger' && (
              <AlertTriangle className="w-5 h-5 text-red-400" />
            )}
            <h2 className="text-lg font-medium text-theme-text-primary">{title}</h2>
          </div>
          <button
            onClick={onClose}
            disabled={isLoading}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors disabled:opacity-50"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Body */}
        <div className="p-4">
          <p className="text-sm text-theme-text-secondary">{message}</p>
        </div>

        {/* Footer */}
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={onClose} disabled={isLoading}>
            {cancelLabel}
          </Button>
          <Button
            onClick={onConfirm}
            disabled={isLoading}
            className={variant === 'danger' ? 'bg-red-600 hover:bg-red-700' : ''}
          >
            {isLoading ? (
              <span className="flex items-center gap-2">
                <span className="w-4 h-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                {confirmLabel}
              </span>
            ) : (
              confirmLabel
            )}
          </Button>
        </div>
      </div>
    </div>
  )
}
