import { Suspense, useEffect, useState } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useMyAudiences } from '@/hooks/useMyAudiences'
import { KeyRound } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { Button } from '@/components/ui/button'
import { MintIngestionKeyDialog } from '@/components/MintIngestionKeyDialog'
import { ApiKeysAdminPage, ApiKeysAdminPageConfig } from '@/components/ApiKeysAdminPage'
import { useAuth } from '@/lib/auth'
import {
  listIngestionApiKeys,
  mintIngestionApiKey,
  revokeIngestionApiKey,
  IngestionApiKeyError,
  MAX_INGESTION_API_KEYS_LIST_LIMIT,
} from '@/lib/ingestion-api-keys-api'
import type { MintApiKeyResponse } from '@/lib/api-keys-shared'

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
  ErrorClass: IngestionApiKeyError,
  listKeys: listIngestionApiKeys,
  mintKey: mintIngestionApiKey,
  revokeKey: revokeIngestionApiKey,
  showAudience: true,
}

/**
 * A non-admin's view of `/admin/ingestion-keys`: mint only, no list/revoke table -- `list_keys`
 * and `revoke_key` stay `AdminUser`-gated server-side, so there is nothing this panel could show
 * for them. Reuses the same self-service mint machinery `/audiences` already built
 * (`useMyAudiences`, `MintIngestionKeyDialog`) instead of a second copy.
 */
function IngestionKeysSelfServicePanel() {
  const { me, selfServiceOff, error, reload } = useMyAudiences()
  const [mintOpen, setMintOpen] = useState(false)

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      void reload()
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const handleMinted = (response: MintApiKeyResponse) => {
    if (response.claimed) {
      void reload()
    }
  }

  // `me !== null` guards against a genuine my-audiences fetch failure: without it, Mint would
  // stay active with a null identity -- wrong prefix, no audience options in the dialog's
  // <select>. Mirrors `AudienceAccessPage.tsx`'s `showMintButton`.
  const showMintButton = me !== null && !selfServiceOff

  return (
    <PageLayout onRefresh={() => void reload()}>
      <div className="p-6 flex flex-col h-full">
        <div className="flex items-center gap-1.5 text-sm text-theme-text-muted mb-4">
          <AppLink href="/admin" className="text-accent-link hover:underline">
            Admin
          </AppLink>
          <span>/</span>
          <span>Ingestion API Keys</span>
        </div>

        <div className="flex items-center justify-between mb-6 gap-4 flex-wrap">
          <div>
            <h1 className="text-2xl font-semibold text-theme-text-primary">Ingestion API Keys</h1>
            <p className="mt-1 text-theme-text-secondary">
              Mint your own write credentials for telemetry ingestion clients.
            </p>
          </div>
          {showMintButton && (
            <Button onClick={() => setMintOpen(true)} className="gap-1.5">
              <KeyRound className="w-4 h-4" />
              Mint Key
            </Button>
          )}
        </div>

        {error && (
          <ErrorBanner
            title="Failed to load your audiences"
            message={error}
            onRetry={() => void reload()}
          />
        )}

        {selfServiceOff && (
          <p className="text-sm text-theme-text-muted mb-4">
            Self-service is disabled on this deployment. Ask an admin to mint a key for you.
          </p>
        )}

        <p className="text-sm text-theme-text-muted">
          Review the audiences you can mint into on{' '}
          <AppLink href="/audiences" className="text-accent-link hover:underline">
            Audience Access
          </AppLink>
          .
        </p>

        <MintIngestionKeyDialog
          open={mintOpen && me !== null}
          prefillAudience={null}
          me={me}
          onClose={() => setMintOpen(false)}
          onMinted={handleMinted}
        />
      </div>
    </PageLayout>
  )
}

export interface IngestionApiKeysPageProps {
  /**
   * Rows per page. Defaults to the server's max list limit; the route renders
   * this component with no props, so only tests ever pass a smaller size (to
   * exercise paging without rendering a full page of rows).
   */
  pageSize?: number
}

function IngestionApiKeysPageContent({ pageSize }: { pageSize: number }) {
  // Called once ahead of the role branch so both the admin and non-admin branches set the
  // document title -- `ApiKeysAdminPage.tsx`'s own `usePageTitle(config.title)` call on the
  // admin branch is a redundant-but-harmless duplicate of the same string, since the hook is
  // idempotent on identical input.
  usePageTitle('Ingestion API Keys')
  const { user } = useAuth()

  return (
    <AuthGuard>
      {user?.is_admin ? (
        <ApiKeysAdminPage config={ingestionApiKeysPageConfig} pageSize={pageSize} />
      ) : (
        <IngestionKeysSelfServicePanel />
      )}
    </AuthGuard>
  )
}

export default function IngestionApiKeysPage({
  pageSize = MAX_INGESTION_API_KEYS_LIST_LIMIT,
}: IngestionApiKeysPageProps) {
  return (
    <Suspense
      fallback={
        <AuthGuard>
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
      <IngestionApiKeysPageContent pageSize={pageSize} />
    </Suspense>
  )
}
