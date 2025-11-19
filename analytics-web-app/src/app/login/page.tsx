'use client'

import { Suspense, useEffect } from 'react'
import { useSearchParams } from 'next/navigation'
import { useAuth } from '@/lib/auth'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { AlertCircle, LogIn } from 'lucide-react'

function LoginContent() {
  const { status, error, login } = useAuth()
  const searchParams = useSearchParams()

  const returnUrl = searchParams.get('return_url') || '/'
  const authError = searchParams.get('error')

  useEffect(() => {
    // If already authenticated, redirect to return URL
    if (status === 'authenticated') {
      window.location.href = returnUrl
    }
  }, [status, returnUrl])

  const handleLogin = () => {
    login(returnUrl)
  }

  if (status === 'loading') {
    return (
      <div className="min-h-screen flex items-center justify-center bg-gray-50">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6">
            <div className="flex flex-col items-center space-y-4">
              <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-gray-900"></div>
              <p className="text-sm text-gray-600">Checking authentication...</p>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  return (
    <div className="min-h-screen flex items-center justify-center bg-gray-50">
      <Card className="w-full max-w-md">
        <CardHeader className="text-center">
          <CardTitle className="text-2xl">Analytics Web App</CardTitle>
          <CardDescription>
            Sign in to access telemetry data and analytics
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {(authError || error) && (
            <div className="bg-red-50 border border-red-200 rounded-md p-4">
              <div className="flex items-start">
                <AlertCircle className="h-5 w-5 text-red-400 mt-0.5 mr-2" />
                <div>
                  <h3 className="text-sm font-medium text-red-800">
                    Authentication Error
                  </h3>
                  <p className="text-sm text-red-700 mt-1">
                    {authError || error}
                  </p>
                </div>
              </div>
            </div>
          )}

          {status === 'error' ? (
            <div className="bg-yellow-50 border border-yellow-200 rounded-md p-4">
              <p className="text-sm text-yellow-800">
                Unable to connect to authentication service. Please try again later.
              </p>
            </div>
          ) : (
            <Button
              onClick={handleLogin}
              className="w-full"
              size="lg"
            >
              <LogIn className="mr-2 h-4 w-4" />
              Sign in with SSO
            </Button>
          )}

          <p className="text-xs text-center text-gray-500 mt-4">
            You will be redirected to your organization&apos;s identity provider.
          </p>
        </CardContent>
      </Card>
    </div>
  )
}

function LoginFallback() {
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

export default function LoginPage() {
  return (
    <Suspense fallback={<LoginFallback />}>
      <LoginContent />
    </Suspense>
  )
}
