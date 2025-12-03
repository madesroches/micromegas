'use client'

import { AlertCircle, AlertTriangle, X } from 'lucide-react'

interface ErrorBannerProps {
  title: string
  message: string
  details?: string
  variant?: 'error' | 'warning'
  onDismiss?: () => void
  onRetry?: () => void
  actions?: React.ReactNode
}

export function ErrorBanner({
  title,
  message,
  details,
  variant = 'error',
  onDismiss,
  onRetry,
  actions,
}: ErrorBannerProps) {
  const isWarning = variant === 'warning'
  const bgColor = isWarning ? 'bg-accent-warning/10' : 'bg-accent-error/10'
  const borderColor = isWarning ? 'border-accent-warning/30' : 'border-accent-error/30'
  const iconColor = isWarning ? 'text-accent-warning' : 'text-accent-error'
  const titleColor = isWarning ? 'text-accent-warning' : 'text-accent-error'
  const messageColor = isWarning ? 'text-accent-warning/80' : 'text-accent-error/80'

  const Icon = isWarning ? AlertTriangle : AlertCircle

  return (
    <div className={`flex items-start gap-3 p-3.5 ${bgColor} border ${borderColor} rounded-lg mb-4`}>
      <Icon className={`w-5 h-5 ${iconColor} flex-shrink-0 mt-0.5`} />
      <div className="flex-1 min-w-0">
        <div className={`text-sm font-semibold ${titleColor}`}>{title}</div>
        <div className={`text-sm ${messageColor} mt-1`}>{message}</div>
        {details && (
          <div className="mt-2 px-2.5 py-1.5 bg-black/20 rounded text-xs font-mono text-theme-text-muted">
            {details}
          </div>
        )}
        {(onRetry || actions) && (
          <div className="flex gap-2 mt-3">
            {onRetry && (
              <button
                onClick={onRetry}
                className={`px-3 py-1.5 text-xs rounded transition-colors ${
                  isWarning
                    ? 'bg-accent-warning text-black hover:opacity-80'
                    : 'bg-accent-error text-white hover:opacity-80'
                }`}
              >
                Retry
              </button>
            )}
            {actions}
          </div>
        )}
      </div>
      {onDismiss && (
        <button
          onClick={onDismiss}
          className={`p-1 rounded transition-colors flex-shrink-0 ${
            isWarning
              ? 'text-accent-warning/60 hover:text-accent-warning hover:bg-accent-warning/20'
              : 'text-accent-error/60 hover:text-accent-error hover:bg-accent-error/20'
          }`}
        >
          <X className="w-4 h-4" />
        </button>
      )}
    </div>
  )
}
