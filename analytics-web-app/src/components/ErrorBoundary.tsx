'use client'

import React from 'react'
import { ApiErrorException } from '@/lib/api'
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
        <div className="flex flex-col items-center justify-center min-h-[200px] p-4">
          <div className="text-red-600 mb-2">
            <svg className="w-8 h-8" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 8v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
          </div>
          <h2 className="text-lg font-semibold text-gray-900 mb-1">Something went wrong</h2>
          <p className="text-sm text-gray-600 text-center max-w-md">
            {this.state.error?.message || 'An unexpected error occurred. Please try refreshing the page.'}
          </p>
          <button
            className="mt-4 px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 transition-colors"
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

  const handleError = React.useCallback((error: unknown) => {
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
  }, [toast])

  return handleError
}