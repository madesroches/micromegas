import { Suspense } from 'react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { ApiKeysAdminPage, ApiKeysAdminPageConfig } from '@/components/ApiKeysAdminPage'
import {
  listAnalyticsApiKeys,
  mintAnalyticsApiKey,
  revokeAnalyticsApiKey,
  AnalyticsApiKeyError,
  MAX_ANALYTICS_API_KEYS_LIST_LIMIT,
} from '@/lib/analytics-api-keys-api'

// Module-level constant (not created inline in JSX) so its identity is
// stable across renders — `ApiKeysAdminPage`'s `loadKeys` depends on it, and
// a fresh object every render would retrigger the load-on-mount effect.
const analyticsApiKeysPageConfig: ApiKeysAdminPageConfig = {
  title: 'Analytics API Keys',
  subtitle: 'Manage read credentials for FlightSQL/analytics access.',
  mintDialogTitle: 'Mint Analytics API Key',
  namePlaceholder: 'e.g. grafana-datasource',
  emptyStateText: 'No analytics API keys yet.',
  loadErrorMessage: 'Failed to load analytics API keys',
  revokeConfirmMessage: (name) =>
    `Are you sure you want to revoke "${name}"? Any client using this key will lose access.`,
  ErrorClass: AnalyticsApiKeyError,
  listKeys: listAnalyticsApiKeys,
  mintKey: mintAnalyticsApiKey,
  revokeKey: revokeAnalyticsApiKey,
}

export interface AnalyticsApiKeysPageProps {
  /**
   * Rows per page. Defaults to the server's max list limit; the route renders
   * this component with no props, so only tests ever pass a smaller size (to
   * exercise paging without rendering a full page of rows).
   */
  pageSize?: number
}

export default function AnalyticsApiKeysPage({
  pageSize = MAX_ANALYTICS_API_KEYS_LIST_LIMIT,
}: AnalyticsApiKeysPageProps = {}) {
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
      <ApiKeysAdminPage config={analyticsApiKeysPageConfig} pageSize={pageSize} />
    </Suspense>
  )
}
