import { Suspense, useEffect } from 'react'
import { useSearchParams } from 'react-router-dom'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useAuth } from '@/lib/auth'
import { getConfig } from '@/lib/config'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { AlertCircle, LogIn } from 'lucide-react'
import { MicromegasLogo } from '@/components/MicromegasLogo'

function LoginContent() {
  usePageTitle('Login')
  const { status, error, login } = useAuth()
  const [searchParams] = useSearchParams()

  const returnUrlParam = searchParams.get('return_url')
  const authError = searchParams.get('error')

  // Build the return URL, ensuring it includes the base path if it's a relative path
  const { basePath } = getConfig()
  const returnUrl = returnUrlParam
    ? (returnUrlParam.startsWith('/') && !returnUrlParam.startsWith(basePath)
      ? `${basePath}${returnUrlParam}`
      : returnUrlParam)
    : `${basePath}/processes`

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
      <div className="min-h-screen flex items-center justify-center bg-app-bg">
        <Card className="w-full max-w-md bg-app-panel border-theme-border">
          <CardContent className="pt-6">
            <div className="flex flex-col items-center space-y-4">
              <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-brand-blue"></div>
              <p className="text-sm text-theme-text-secondary">Checking authentication...</p>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  return (
    <div className="min-h-screen flex items-center justify-center bg-app-bg">
      <Card className="w-full max-w-md bg-app-panel border-theme-border">
        <CardHeader className="text-center space-y-4">
          <div className="flex justify-center">
            <MicromegasLogo size="lg" />
          </div>
          <div>
            <CardTitle className="text-xl text-theme-text-primary">Analytics</CardTitle>
            <CardDescription className="text-theme-text-secondary">
              Sign in to access telemetry data and analytics
            </CardDescription>
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
          {(authError || error) && (
            <div className="bg-accent-error/10 border border-accent-error/30 rounded-md p-4">
              <div className="flex items-start">
                <AlertCircle className="h-5 w-5 text-accent-error mt-0.5 mr-2" />
                <div>
                  <h3 className="text-sm font-medium text-accent-error">
                    Authentication Error
                  </h3>
                  <p className="text-sm text-accent-error/80 mt-1">
                    {authError || error}
                  </p>
                </div>
              </div>
            </div>
          )}

          {status === 'error' ? (
            <div className="bg-accent-warning/10 border border-accent-warning/30 rounded-md p-4">
              <p className="text-sm text-accent-warning">
                Unable to connect to authentication service. Please try again later.
              </p>
            </div>
          ) : (
            <Button
              onClick={handleLogin}
              className="w-full bg-brand-blue hover:bg-brand-blue-dark text-white"
              size="lg"
            >
              <LogIn className="mr-2 h-4 w-4" />
              Sign in with SSO
            </Button>
          )}

          <p className="text-xs text-center text-theme-text-muted mt-4">
            You will be redirected to your organization&apos;s identity provider.
          </p>
        </CardContent>
      </Card>
    </div>
  )
}

function LoginFallback() {
  return (
    <div className="min-h-screen flex items-center justify-center bg-app-bg">
      <Card className="w-full max-w-md bg-app-panel border-theme-border">
        <CardContent className="pt-6">
          <div className="flex flex-col items-center space-y-4">
            <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-brand-blue"></div>
            <p className="text-sm text-theme-text-secondary">Loading...</p>
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
