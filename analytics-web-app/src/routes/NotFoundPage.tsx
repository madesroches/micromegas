import { AppLink } from '@/components/AppLink'
import { AlertCircle } from 'lucide-react'
import { usePageTitle } from '@/hooks/usePageTitle'

export default function NotFoundPage() {
  usePageTitle('Page Not Found')
  return (
    <div className="min-h-screen bg-app-bg flex items-center justify-center p-6">
      <div className="text-center">
        <AlertCircle className="w-16 h-16 text-accent-error mx-auto mb-4" />
        <h1 className="text-3xl font-semibold text-theme-text-primary mb-2">Page Not Found</h1>
        <p className="text-theme-text-secondary mb-6">
          The page you're looking for doesn't exist or has been moved.
        </p>
        <AppLink
          href="/processes"
          className="inline-flex items-center px-4 py-2 bg-accent-link text-white rounded-md hover:bg-accent-link/90 transition-colors"
        >
          Go to Processes
        </AppLink>
      </div>
    </div>
  )
}
