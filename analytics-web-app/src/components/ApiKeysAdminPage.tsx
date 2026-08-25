// Shared content for the analytics/ingestion API-key admin pages
// (`AnalyticsApiKeysPage.tsx` and `IngestionApiKeysPage.tsx`). Both pages are
// identical in markup and behavior apart from copy (title/subtitle/
// placeholder/revoke-confirm wording) and which API-client functions/error
// class they call — all supplied via `config` — plus the `pageSize` prop each
// page defaults to its own server max limit.
// Each page keeps its own outer `<Suspense fallback={<AuthGuard>...}>`
// wrapper (see `AnalyticsApiKeysPage.tsx`); this component owns the
// `<AuthGuard><PageLayout>` wrapping for the loaded content itself, since
// that part is identical between the two pages.
import { useState, useEffect, useCallback } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Plus, Trash2, KeyRound } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { Button } from '@/components/ui/button'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { MintedKeyBanner } from '@/components/MintedKeyBanner'
import type {
  ApiKeyErrorConstructor,
  ApiKeyListEntry,
  MintApiKeyResponse,
} from '@/lib/api-keys-shared'

function formatDate(iso: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString()
}

export interface ApiKeysAdminPageConfig {
  /** Used for the document title, breadcrumb trailing segment, and H1 heading. */
  title: string
  subtitle: string
  mintDialogTitle: string
  namePlaceholder: string
  emptyStateText: string
  /** Shown as the page-level error banner when the initial/refresh list load fails. */
  loadErrorMessage: string
  /** `revokeConfirmMessage(name)` — the target key's name may be undefined while the dialog is closing. */
  revokeConfirmMessage: (name: string | undefined) => string
  ErrorClass: ApiKeyErrorConstructor
  listKeys: (includeRevoked: boolean, offset: number, limit: number) => Promise<ApiKeyListEntry[]>
  mintKey: (name: string, audience?: string) => Promise<MintApiKeyResponse>
  revokeKey: (keyId: string) => Promise<unknown>
  /**
   * Shows an "Audience" table column and an audience input in the mint dialog
   * (#1372, AbAC Stage 4). Ingestion keys carry a write audience; analytics
   * keys never do, so the analytics page leaves this unset.
   */
  showAudience?: boolean
}

export interface ApiKeysAdminPageProps {
  config: ApiKeysAdminPageConfig
  /**
   * Rows per page: the `limit` sent on every list call, the step the
   * Previous/Next buttons move `offset` by, and the row count a full page is
   * recognized by. One value for all three — a list call that asked for a
   * different limit than the paging UI compares against would show a Next
   * button onto a page that doesn't exist (or hide one that does).
   *
   * Each page defaults it to the server's max limit; it's a prop so callers
   * (tests included) can page at a size that doesn't mean rendering 500 rows.
   */
  pageSize: number
}

export function ApiKeysAdminPage({ config, pageSize }: ApiKeysAdminPageProps) {
  usePageTitle(config.title)

  const [keys, setKeys] = useState<ApiKeyListEntry[]>([])
  const [offset, setOffset] = useState(0)
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [showMintForm, setShowMintForm] = useState(false)
  const [mintName, setMintName] = useState('')
  const [mintAudience, setMintAudience] = useState('')
  const [isMinting, setIsMinting] = useState(false)
  const [mintError, setMintError] = useState<string | null>(null)
  // The cleartext key is shown exactly once, right after minting — never
  // persisted client-side, never refetchable.
  const [mintedKey, setMintedKey] = useState<string | null>(null)
  const [revokeTarget, setRevokeTarget] = useState<ApiKeyListEntry | null>(null)
  const [isRevoking, setIsRevoking] = useState(false)

  const loadKeys = useCallback(async () => {
    setIsLoading(true)
    setError(null)
    try {
      const data = await config.listKeys(true, offset, pageSize)
      setKeys(data)
    } catch (err) {
      setError(err instanceof config.ErrorClass ? err.message : config.loadErrorMessage)
    } finally {
      setIsLoading(false)
    }
  }, [offset, pageSize, config])

  const goToPreviousPage = () => {
    setOffset((current) => Math.max(0, current - pageSize))
  }

  const goToNextPage = () => {
    setOffset((current) => current + pageSize)
  }

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level — see react-hooks/set-state-in-effect
    void (async () => {
      await loadKeys()
    })()
  }, [loadKeys])

  const openMintForm = () => {
    setMintName('')
    setMintAudience(config.showAudience ? 'public' : '')
    setMintError(null)
    setShowMintForm(true)
  }

  const handleMint = async () => {
    setIsMinting(true)
    setMintError(null)
    try {
      const result = await config.mintKey(mintName.trim(), mintAudience.trim() || undefined)
      setShowMintForm(false)
      setMintedKey(result.key)
      // A key minted while on a later page would otherwise land on page 1
      // (newest-first ordering) and never show up in the visible table.
      // Resetting to page 1 changes `loadKeys`'s dep and triggers the
      // load-on-offset-change effect; already on page 1, so refetch directly.
      if (offset !== 0) {
        setOffset(0)
      } else {
        await loadKeys()
      }
    } catch (err) {
      setMintError(err instanceof config.ErrorClass ? err.message : 'Failed to mint key')
    } finally {
      setIsMinting(false)
    }
  }

  const handleRevoke = async () => {
    if (!revokeTarget) return
    setIsRevoking(true)
    try {
      await config.revokeKey(revokeTarget.key_id)
      setRevokeTarget(null)
      await loadKeys()
    } catch (err) {
      setError(err instanceof config.ErrorClass ? err.message : 'Failed to revoke key')
      setRevokeTarget(null)
    } finally {
      setIsRevoking(false)
    }
  }

  return (
    <AuthGuard requireAdmin>
      <PageLayout onRefresh={loadKeys}>
        <div className="p-6 flex flex-col h-full">
          <div className="flex items-center gap-1.5 text-sm text-theme-text-muted mb-4">
            <AppLink href="/admin" className="text-accent-link hover:underline">
              Admin
            </AppLink>
            <span>/</span>
            <span>{config.title}</span>
          </div>

          <div className="flex items-center justify-between mb-6">
            <div>
              <h1 className="text-2xl font-semibold text-theme-text-primary">{config.title}</h1>
              <p className="mt-1 text-theme-text-secondary">{config.subtitle}</p>
            </div>
            <Button onClick={openMintForm} className="gap-1.5">
              <Plus className="w-4 h-4" />
              Mint Key
            </Button>
          </div>

          {error && (
            <ErrorBanner title="Error" message={error} onDismiss={() => setError(null)} />
          )}

          {mintedKey && (
            <MintedKeyBanner keyValue={mintedKey} onDismiss={() => setMintedKey(null)} />
          )}

          {showMintForm && (
            <div className="fixed inset-0 z-50 flex items-center justify-center">
              <div
                className="absolute inset-0 bg-black/50"
                onClick={() => !isMinting && setShowMintForm(false)}
              />
              <div className="relative w-full max-w-md bg-app-panel border border-theme-border rounded-lg shadow-xl">
                <div className="px-4 py-3 border-b border-theme-border">
                  <h2 className="text-lg font-medium text-theme-text-primary">{config.mintDialogTitle}</h2>
                </div>
                <div className="p-4 space-y-4">
                  {mintError && (
                    <div className="p-3 bg-accent-error/10 border border-accent-error/30 rounded-lg text-sm text-accent-error">
                      {mintError}
                    </div>
                  )}
                  <div>
                    <label className="block text-sm font-medium text-theme-text-secondary mb-1">
                      Name
                    </label>
                    <input
                      type="text"
                      className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
                      placeholder={config.namePlaceholder}
                      value={mintName}
                      onChange={(e) => setMintName(e.target.value)}
                      autoFocus
                    />
                  </div>
                  {config.showAudience && (
                    <div>
                      <label className="block text-sm font-medium text-theme-text-secondary mb-1">
                        Audience
                      </label>
                      <input
                        type="text"
                        className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm text-theme-text-primary placeholder:text-theme-text-muted outline-hidden focus:border-accent-link"
                        placeholder="team-alpha"
                        value={mintAudience}
                        onChange={(e) => setMintAudience(e.target.value)}
                      />
                      <p className="mt-1 text-xs text-theme-text-muted">
                        The write audience this key is scoped to. "public" is readable by every
                        authenticated principal; use a named audience to restrict it.
                      </p>
                    </div>
                  )}
                </div>
                <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
                  <Button variant="outline" onClick={() => setShowMintForm(false)} disabled={isMinting}>
                    Cancel
                  </Button>
                  <Button
                    onClick={handleMint}
                    disabled={
                      isMinting ||
                      !mintName.trim() ||
                      (config.showAudience && !mintAudience.trim())
                    }
                  >
                    {isMinting ? (
                      <span className="flex items-center gap-2">
                        <span className="w-4 h-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                        Minting...
                      </span>
                    ) : (
                      'Mint'
                    )}
                  </Button>
                </div>
              </div>
            </div>
          )}

          <ConfirmDialog
            isOpen={revokeTarget !== null}
            onClose={() => setRevokeTarget(null)}
            onConfirm={handleRevoke}
            title="Revoke API Key"
            message={config.revokeConfirmMessage(revokeTarget?.name)}
            confirmLabel="Revoke"
            isLoading={isRevoking}
            variant="danger"
          />

          {isLoading ? (
            <div className="flex-1 flex items-center justify-center">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading keys...</span>
              </div>
            </div>
          ) : keys.length === 0 && offset === 0 ? (
            <div className="flex-1 flex flex-col items-center justify-center text-center">
              <KeyRound className="w-10 h-10 text-theme-text-muted opacity-40 mb-3" />
              <p className="text-theme-text-muted mb-4">{config.emptyStateText}</p>
              <Button onClick={openMintForm} className="gap-1.5">
                <Plus className="w-4 h-4" />
                Mint Key
              </Button>
            </div>
          ) : keys.length === 0 ? (
            <div className="flex-1 flex flex-col items-center justify-center text-center">
              <KeyRound className="w-10 h-10 text-theme-text-muted opacity-40 mb-3" />
              <p className="text-theme-text-muted mb-4">No more keys on this page.</p>
              <Button variant="outline" onClick={goToPreviousPage} className="gap-1.5">
                Previous
              </Button>
            </div>
          ) : (
            <div className="border border-theme-border rounded-lg overflow-hidden">
              <table className="w-full border-collapse">
                <thead className="bg-app-panel">
                  <tr>
                    <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Name
                    </th>
                    <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Created
                    </th>
                    <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Last Used
                    </th>
                    {config.showAudience && (
                      <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                        Audience
                      </th>
                    )}
                    <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Status
                    </th>
                    <th className="text-right p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Actions
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {keys.map((key) => (
                    <tr key={key.key_id} className="border-t border-theme-border hover:bg-accent-link/5">
                      <td className="p-2.5 px-4">
                        <span className="text-theme-text-primary font-medium">{key.name}</span>
                        <span className="text-theme-text-muted ml-2 text-xs">by {key.created_by}</span>
                      </td>
                      <td className="p-2.5 px-4 text-theme-text-secondary text-sm">
                        {formatDate(key.created_at)}
                      </td>
                      <td className="p-2.5 px-4 text-theme-text-secondary text-sm">
                        {formatDate(key.last_used_at)}
                      </td>
                      {config.showAudience && (
                        <td className="p-2.5 px-4 text-theme-text-secondary text-sm">
                          {key.audience ?? '—'}
                        </td>
                      )}
                      <td className="p-2.5 px-4">
                        {key.revoked_at ? (
                          <span className="inline-flex items-center px-2 py-0.5 bg-red-500/15 text-red-400 rounded-sm text-xs font-medium">
                            Revoked
                          </span>
                        ) : (
                          <span className="inline-flex items-center px-2 py-0.5 bg-green-500/15 text-green-500 rounded-sm text-xs font-medium">
                            Active
                          </span>
                        )}
                      </td>
                      <td className="p-2.5 px-4 text-right">
                        {!key.revoked_at && (
                          <button
                            onClick={() => setRevokeTarget(key)}
                            className="p-1.5 rounded-sm text-theme-text-muted hover:text-red-400 hover:bg-red-400/10 transition-colors"
                            title="Revoke"
                            aria-label={`Revoke ${key.name}`}
                          >
                            <Trash2 className="w-4 h-4" />
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {(offset > 0 || keys.length === pageSize) && (
                <div className="flex items-center justify-between border-t border-theme-border bg-app-panel px-4 py-2.5">
                  {offset > 0 ? (
                    <Button variant="outline" size="sm" onClick={goToPreviousPage}>
                      Previous
                    </Button>
                  ) : (
                    <span />
                  )}
                  {keys.length === pageSize ? (
                    <Button variant="outline" size="sm" onClick={goToNextPage}>
                      Next
                    </Button>
                  ) : (
                    <span />
                  )}
                </div>
              )}
            </div>
          )}
        </div>
      </PageLayout>
    </AuthGuard>
  )
}
