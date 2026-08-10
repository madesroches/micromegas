import { Suspense, useState, useEffect, useCallback } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Plus, Trash2, Copy, Check, KeyRound } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { Button } from '@/components/ui/button'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import {
  listAnalyticsApiKeys,
  mintAnalyticsApiKey,
  revokeAnalyticsApiKey,
  AnalyticsApiKeyListEntry,
  AnalyticsApiKeyError,
} from '@/lib/analytics-api-keys-api'

function formatDate(iso: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString()
}

function AnalyticsApiKeysPageContent() {
  usePageTitle('Analytics API Keys')

  const [keys, setKeys] = useState<AnalyticsApiKeyListEntry[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [showMintForm, setShowMintForm] = useState(false)
  const [mintName, setMintName] = useState('')
  const [isMinting, setIsMinting] = useState(false)
  const [mintError, setMintError] = useState<string | null>(null)
  // The cleartext key is shown exactly once, right after minting — never
  // persisted client-side, never refetchable.
  const [mintedKey, setMintedKey] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [revokeTarget, setRevokeTarget] = useState<AnalyticsApiKeyListEntry | null>(null)
  const [isRevoking, setIsRevoking] = useState(false)

  const loadKeys = useCallback(async () => {
    setIsLoading(true)
    setError(null)
    try {
      const data = await listAnalyticsApiKeys()
      setKeys(data)
    } catch (err) {
      setError(err instanceof AnalyticsApiKeyError ? err.message : 'Failed to load analytics API keys')
    } finally {
      setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level — see react-hooks/set-state-in-effect
    void (async () => {
      await loadKeys()
    })()
  }, [loadKeys])

  const openMintForm = () => {
    setMintName('')
    setMintError(null)
    setShowMintForm(true)
  }

  const handleMint = async () => {
    setIsMinting(true)
    setMintError(null)
    try {
      const result = await mintAnalyticsApiKey(mintName.trim())
      setShowMintForm(false)
      setMintedKey(result.key)
      setCopied(false)
      await loadKeys()
    } catch (err) {
      setMintError(err instanceof AnalyticsApiKeyError ? err.message : 'Failed to mint key')
    } finally {
      setIsMinting(false)
    }
  }

  const handleCopyKey = async () => {
    if (!mintedKey) return
    try {
      await navigator.clipboard.writeText(mintedKey)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // Clipboard access can fail (permissions, insecure context) — the key
      // is still visible and selectable, so this is a soft failure.
    }
  }

  const handleRevoke = async () => {
    if (!revokeTarget) return
    setIsRevoking(true)
    try {
      await revokeAnalyticsApiKey(revokeTarget.key_id)
      setRevokeTarget(null)
      await loadKeys()
    } catch (err) {
      setError(err instanceof AnalyticsApiKeyError ? err.message : 'Failed to revoke key')
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
            <span>Analytics API Keys</span>
          </div>

          <div className="flex items-center justify-between mb-6">
            <div>
              <h1 className="text-2xl font-semibold text-theme-text-primary">Analytics API Keys</h1>
              <p className="mt-1 text-theme-text-secondary">
                Manage read credentials for FlightSQL/analytics access.
              </p>
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
            <div className="mb-4 p-4 rounded-lg border border-accent-warning/40 bg-accent-warning/10">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="text-sm font-semibold text-theme-text-primary mb-1">
                    Key minted — copy it now, it won't be shown again
                  </div>
                  <code className="block break-all text-sm font-mono text-theme-text-primary bg-app-bg px-2.5 py-1.5 rounded-sm border border-theme-border">
                    {mintedKey}
                  </code>
                </div>
                <button
                  onClick={() => setMintedKey(null)}
                  className="shrink-0 p-1.5 rounded-sm text-theme-text-muted hover:text-theme-text-primary hover:bg-theme-border transition-colors"
                  aria-label="Dismiss"
                >
                  ×
                </button>
              </div>
              <Button variant="outline" onClick={handleCopyKey} className="mt-3 gap-1.5">
                {copied ? <Check className="w-4 h-4 text-green-500" /> : <Copy className="w-4 h-4" />}
                {copied ? 'Copied' : 'Copy key'}
              </Button>
            </div>
          )}

          {showMintForm && (
            <div className="fixed inset-0 z-50 flex items-center justify-center">
              <div
                className="absolute inset-0 bg-black/50"
                onClick={() => !isMinting && setShowMintForm(false)}
              />
              <div className="relative w-full max-w-md bg-app-panel border border-theme-border rounded-lg shadow-xl">
                <div className="px-4 py-3 border-b border-theme-border">
                  <h2 className="text-lg font-medium text-theme-text-primary">Mint Analytics API Key</h2>
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
                      placeholder="e.g. grafana-datasource"
                      value={mintName}
                      onChange={(e) => setMintName(e.target.value)}
                      autoFocus
                    />
                  </div>
                </div>
                <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
                  <Button variant="outline" onClick={() => setShowMintForm(false)} disabled={isMinting}>
                    Cancel
                  </Button>
                  <Button onClick={handleMint} disabled={isMinting || !mintName.trim()}>
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
            message={`Are you sure you want to revoke "${revokeTarget?.name}"? Any client using this key will lose access.`}
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
          ) : keys.length === 0 ? (
            <div className="flex-1 flex flex-col items-center justify-center text-center">
              <KeyRound className="w-10 h-10 text-theme-text-muted opacity-40 mb-3" />
              <p className="text-theme-text-muted mb-4">No analytics API keys yet.</p>
              <Button onClick={openMintForm} className="gap-1.5">
                <Plus className="w-4 h-4" />
                Mint Key
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
            </div>
          )}
        </div>
      </PageLayout>
    </AuthGuard>
  )
}

export default function AnalyticsApiKeysPage() {
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
      <AnalyticsApiKeysPageContent />
    </Suspense>
  )
}
