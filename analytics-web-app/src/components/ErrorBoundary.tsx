'use client'

import React from 'react'
import { useRouter } from 'next/navigation'
import { ApiErrorException, AuthenticationError } from '@/lib/api'
import { useToast } from '@/lib/use-toast'

interface ErrorBoundaryProps {
  children: React.ReactNode
  fallback?: React.ReactNode
}

interface ErrorBoundaryState {
  hasError: boolean
  error?: Error
}

export class ErrorBoundary extends React.Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props)
    this.state = { hasError: false }
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error }
  }

  componentDidCatch(error: Error, errorInfo: React.ErrorInfo) {
    console.error('ErrorBoundary caught an error:', error, errorInfo)
  }

  render() {
    if (this.state.hasError) {
      if (this.props.fallback) {
        return this.props.fallback
      }

      return (
        <div className="flex flex-col items-center justify-center min-h-[200px] p-6 bg-[#1a1f26] border border-[#2f3540] rounded-lg">
          <div className="text-red-500 mb-4">
            <svg className="w-12 h-12" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 8v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
          </div>
          <h2 className="text-lg font-semibold text-gray-200 mb-2">Something went wrong</h2>
          <p className="text-sm text-gray-400 text-center max-w-md mb-4">
            {this.state.error?.message || 'An unexpected error occurred. Please try refreshing the page.'}
          </p>
          <button
            className="px-4 py-2 bg-blue-500 text-white rounded-md hover:bg-blue-600 transition-colors"
            onClick={() => this.setState({ hasError: false, error: undefined })}
          >
            Try again
          </button>
        </div>
      )
    }

    return this.props.children
  }
}

// Hook to use in components for API error handling
export function useApiErrorHandler() {
  const { toast } = useToast()
  const router = useRouter()

  const handleError = React.useCallback((error: unknown) => {
    // Handle authentication errors by redirecting to login
    if (error instanceof AuthenticationError) {
      const returnUrl = encodeURIComponent(window.location.pathname)
      router.push(`/login?return_url=${returnUrl}`)
      return
    }

    let title = 'Error'
    let description = 'An unexpected error occurred'

    if (error instanceof ApiErrorException) {
      title = error.apiError.type.replace(/([A-Z])/g, ' $1').replace(/^./, str => str.toUpperCase())
      description = error.apiError.message
      if (error.apiError.details) {
        description += `: ${error.apiError.details}`
      }
    } else if (error instanceof Error) {
      description = error.message
    } else if (typeof error === 'string') {
      description = error
    }

    toast({
      title,
      description,
      variant: "destructive",
    })
  }, [toast, router])

  return handleError
}