import { useEffect } from 'react'
import { useLocation } from 'react-router-dom'
import { useAuth } from '@/lib/auth'
import { getConfig } from '@/lib/config'
import { Card, CardContent } from '@/components/ui/card'
import { AlertCircle } from 'lucide-react'

interface AuthGuardProps {
  children: React.ReactNode
  requireAdmin?: boolean
}

export function AuthGuard({ children, requireAdmin }: AuthGuardProps) {
  const { status, error, user } = useAuth()
  const location = useLocation()
  const pathname = location.pathname

  useEffect(() => {
    if (status === 'unauthenticated') {
      const returnUrl = encodeURIComponent(pathname)
      // Use runtime base path for redirect to login page
      const { basePath } = getConfig()
      window.location.href = `${basePath}/login?return_url=${returnUrl}`
    }
  }, [status, pathname])

  if (status === 'loading') {
    return (
      <div className="min-h-screen flex items-center justify-center bg-gray-50">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6">
            <div className="flex flex-col items-center space-y-4">
              <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-gray-900"></div>
              <p className="text-sm text-gray-600">Loading...</p>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  if (status === 'error') {
    return (
      <div className="min-h-screen flex items-center justify-center bg-gray-50">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6">
            <div className="flex flex-col items-center space-y-4">
              <AlertCircle className="h-12 w-12 text-red-500" />
              <h2 className="text-lg font-semibold">Service Unavailable</h2>
              <p className="text-sm text-gray-600 text-center">
                Unable to connect to the authentication service.
              </p>
              {error && (
                <p className="text-xs text-gray-500 text-center">
                  Error: {error}
                </p>
              )}
              <button
                onClick={() => window.location.reload()}
                className="text-sm text-blue-600 hover:underline"
              >
                Retry
              </button>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  if (status === 'unauthenticated') {
    // Will redirect in useEffect, show loading in the meantime
    return (
      <div className="min-h-screen flex items-center justify-center bg-gray-50">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6">
            <div className="flex flex-col items-center space-y-4">
              <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-gray-900"></div>
              <p className="text-sm text-gray-600">Redirecting to login...</p>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  if (requireAdmin && !user?.is_admin) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-gray-50">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6">
            <div className="flex flex-col items-center space-y-4">
              <AlertCircle className="h-12 w-12 text-red-500" />
              <h2 className="text-lg font-semibold">Access Denied</h2>
              <p className="text-sm text-gray-600 text-center">
                This page requires admin access.
              </p>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  return <>{children}</>
}
