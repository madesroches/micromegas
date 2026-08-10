import { Suspense } from 'react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { ApiKeysAdminPage, ApiKeysAdminPageConfig } from '@/components/ApiKeysAdminPage'
import {
  listIngestionApiKeys,
  mintIngestionApiKey,
  revokeIngestionApiKey,
  IngestionApiKeyError,
  MAX_INGESTION_API_KEYS_LIST_LIMIT,
} from '@/lib/ingestion-api-keys-api'

// Module-level constant (not created inline in JSX) so its identity is
// stable across renders — `ApiKeysAdminPage`'s `loadKeys` depends on it, and
// a fresh object every render would retrigger the load-on-mount effect.
const ingestionApiKeysPageConfig: ApiKeysAdminPageConfig = {
  title: 'Ingestion API Keys',
  subtitle: 'Manage write credentials for telemetry ingestion clients.',
  mintDialogTitle: 'Mint Ingestion API Key',
  namePlaceholder: 'e.g. game-client-42',
  emptyStateText: 'No ingestion API keys yet.',
  loadErrorMessage: 'Failed to load ingestion API keys',
  revokeConfirmMessage: (name) =>
    `Are you sure you want to revoke "${name}"? Any client using this key will lose access once the revocation propagates.`,
  maxListLimit: MAX_INGESTION_API_KEYS_LIST_LIMIT,
  ErrorClass: IngestionApiKeyError,
  listKeys: listIngestionApiKeys,
  mintKey: mintIngestionApiKey,
  revokeKey: revokeIngestionApiKey,
}

export default function IngestionApiKeysPage() {
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
      <ApiKeysAdminPage config={ingestionApiKeysPageConfig} />
    </Suspense>
  )
}
