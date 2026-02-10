import { Suspense } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Database, Download, Upload } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'

function AdminPageContent() {
  usePageTitle('Admin')

  return (
    <AuthGuard requireAdmin>
      <PageLayout>
        <div className="p-6 flex flex-col h-full">
          <div className="mb-6">
            <h1 className="text-2xl font-semibold text-theme-text-primary">Admin</h1>
            <p className="mt-1 text-theme-text-secondary">
              System administration and data management tools.
            </p>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
            <AppLink href="/admin/data-sources" className="block">
              <div className="p-6 rounded-xl border border-theme-border bg-app-panel hover:border-accent-link hover:bg-app-card transition-colors">
                <div className="w-11 h-11 rounded-lg flex items-center justify-center mb-4 bg-green-500/15 text-green-500">
                  <Database className="w-6 h-6" />
                </div>
                <h3 className="text-base font-semibold text-theme-text-primary mb-1.5">Data Sources</h3>
                <p className="text-sm text-theme-text-muted leading-relaxed">
                  Manage FlightSQL server connections used for queries and analytics.
                </p>
              </div>
            </AppLink>

            <AppLink href="/admin/export-screens" className="block">
              <div className="p-6 rounded-xl border border-theme-border bg-app-panel hover:border-accent-link hover:bg-app-card transition-colors">
                <div className="w-11 h-11 rounded-lg flex items-center justify-center mb-4 bg-accent-link/15 text-accent-link">
                  <Download className="w-6 h-6" />
                </div>
                <h3 className="text-base font-semibold text-theme-text-primary mb-1.5">Export Screens</h3>
                <p className="text-sm text-theme-text-muted leading-relaxed">
                  Download screen configurations as a JSON file for backup or transfer to another environment.
                </p>
              </div>
            </AppLink>

            <AppLink href="/admin/import-screens" className="block">
              <div className="p-6 rounded-xl border border-theme-border bg-app-panel hover:border-accent-link hover:bg-app-card transition-colors">
                <div className="w-11 h-11 rounded-lg flex items-center justify-center mb-4 bg-yellow-500/15 text-yellow-500">
                  <Upload className="w-6 h-6" />
                </div>
                <h3 className="text-base font-semibold text-theme-text-primary mb-1.5">Import Screens</h3>
                <p className="text-sm text-theme-text-muted leading-relaxed">
                  Upload a screens export file to restore or migrate screen configurations into this environment.
                </p>
              </div>
            </AppLink>
          </div>
        </div>
      </PageLayout>
    </AuthGuard>
  )
}

export default function AdminPage() {
  return (
    <Suspense
      fallback={
        <AuthGuard requireAdmin>
          <PageLayout>
            <div className="p-6">
              <div className="flex items-center justify-center h-64">
                <div className="animate-spin rounded-full h-8 w-8 border-2 border-accent-link border-t-transparent" />
              </div>
            </div>
          </PageLayout>
        </AuthGuard>
      }
    >
      <AdminPageContent />
    </Suspense>
  )
}
