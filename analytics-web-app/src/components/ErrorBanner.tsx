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
  const bgColor = isWarning ? 'bg-yellow-500/10' : 'bg-red-500/10'
  const borderColor = isWarning ? 'border-yellow-500/30' : 'border-red-500/30'
  const iconColor = isWarning ? 'text-yellow-500' : 'text-red-500'
  const titleColor = isWarning ? 'text-yellow-500' : 'text-red-500'
  const messageColor = isWarning ? 'text-yellow-400' : 'text-red-400'

  const Icon = isWarning ? AlertTriangle : AlertCircle

  return (
    <div className={`flex items-start gap-3 p-3.5 ${bgColor} border ${borderColor} rounded-lg mb-4`}>
      <Icon className={`w-5 h-5 ${iconColor} flex-shrink-0 mt-0.5`} />
      <div className="flex-1 min-w-0">
        <div className={`text-sm font-semibold ${titleColor}`}>{title}</div>
        <div className={`text-sm ${messageColor} mt-1`}>{message}</div>
        {details && (
          <div className="mt-2 px-2.5 py-1.5 bg-black/20 rounded text-xs font-mono text-gray-400">
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
                    ? 'bg-yellow-500 text-black hover:bg-yellow-400'
                    : 'bg-red-500 text-white hover:bg-red-400'
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
              ? 'text-yellow-500/60 hover:text-yellow-500 hover:bg-yellow-500/20'
              : 'text-red-500/60 hover:text-red-500 hover:bg-red-500/20'
          }`}
        >
          <X className="w-4 h-4" />
        </button>
      )}
    </div>
  )
}
