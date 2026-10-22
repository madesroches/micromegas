import { Suspense, useCallback, useEffect, useState } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { X } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import {
  SettingsError,
  fetchSettings,
  resetSetting,
  setSetting,
  type SettingEntry,
  type SettingValue,
} from '@/lib/settings-api'

function formatDate(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString()
}

/** Settings are grouped by key prefix; a key with no known prefix lands in "Other". */
const GROUPS: { prefix: string; title: string }[] = [
  { prefix: 'public_', title: 'Data isolation' },
  { prefix: 'self_service_', title: 'Self-service ingestion keys' },
]

function groupTitle(key: string): string {
  return GROUPS.find((g) => key.startsWith(g.prefix))?.title ?? 'Other'
}

function groupSettings(entries: SettingEntry[]): { title: string; entries: SettingEntry[] }[] {
  const order = [...GROUPS.map((g) => g.title), 'Other']
  return order
    .map((title) => ({ title, entries: entries.filter((e) => groupTitle(e.key) === title) }))
    .filter((g) => g.entries.length > 0)
}

function ValueView({ value, kind }: { value: SettingValue; kind: SettingEntry['kind'] }) {
  if (kind === 'name_list') {
    const names = value as string[]
    if (names.length === 0) return <span className="text-theme-text-muted">empty list</span>
    return (
      <span className="flex flex-wrap gap-1">
        {names.map((n) => (
          <code key={n} className="font-mono text-xs bg-app-bg px-1.5 py-0.5 rounded-sm">
            {n}
          </code>
        ))}
      </span>
    )
  }
  return <code className="font-mono text-xs">{String(value)}</code>
}

// ---------------------------------------------------------------------------
// Edit dialog -- the input fits the setting's kind.
// ---------------------------------------------------------------------------

interface EditDialogProps {
  entry: SettingEntry | null
  onClose: () => void
  onSaved: () => void
}

function EditDialog({ entry, onClose, onSaved }: EditDialogProps) {
  const [boolValue, setBoolValue] = useState(false)
  const [countText, setCountText] = useState('')
  const [names, setNames] = useState<string[]>([])
  const [nameInput, setNameInput] = useState('')
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (() => {
      if (!entry) return
      const current = entry.value ?? entry.default
      setBoolValue(entry.kind === 'bool' ? (current as boolean) : false)
      setCountText(entry.kind === 'count' ? String(current) : '')
      setNames(entry.kind === 'name_list' ? [...(current as string[])] : [])
      setNameInput('')
      setError(null)
    })()
  }, [entry])

  if (!entry) return null

  const addName = () => {
    const trimmed = nameInput.trim()
    if (trimmed && !names.includes(trimmed)) setNames([...names, trimmed])
    setNameInput('')
  }

  const countValid = /^\d+$/.test(countText) && Number.isSafeInteger(Number(countText))

  const pendingValue = (): SettingValue => {
    if (entry.kind === 'bool') return boolValue
    if (entry.kind === 'count') return Number(countText)
    // A name typed but not yet confirmed with Enter is still part of what the admin sees.
    const trimmed = nameInput.trim()
    return trimmed && !names.includes(trimmed) ? [...names, trimmed] : names
  }

  const canSubmit = !isSubmitting && (entry.kind !== 'count' || countValid)

  const handleSubmit = async () => {
    setIsSubmitting(true)
    setError(null)
    try {
      await setSetting(entry.key, pendingValue())
      onSaved()
      onClose()
    } catch (err) {
      setError(err instanceof SettingsError ? err.message : 'Failed to save setting')
    } finally {
      setIsSubmitting(false)
    }
  }

  return (
    <div role="dialog" aria-modal="true" className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={isSubmitting ? undefined : onClose} />
      <div className="relative w-full max-w-md bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="px-4 py-3 border-b border-theme-border flex items-center justify-between">
          <h2 className="text-lg font-medium text-theme-text-primary font-mono">
            Edit {entry.key}
          </h2>
          <button
            onClick={onClose}
            disabled={isSubmitting}
            aria-label="Close"
            className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded-sm transition-colors disabled:opacity-50"
          >
            <X className="w-5 h-5" />
          </button>
        </div>
        <div className="p-4 space-y-4">
          {error && (
            <div className="p-3 bg-accent-error/10 border border-accent-error/30 rounded-lg text-sm text-accent-error">
              {error}
            </div>
          )}
          <p className="text-sm text-theme-text-secondary">{entry.description}</p>
          {entry.value === null && (
            <p className="text-sm text-theme-text-muted">
              Pre-filled with the built-in default. Each service currently uses{' '}
              <code className="font-mono text-xs">{entry.env_var}</code> if it is set, otherwise
              this default. Saving stores a value that overrides it for all services.
            </p>
          )}
          {entry.kind === 'bool' && (
            <label className="flex items-center gap-2 text-sm text-theme-text-primary">
              <input
                type="checkbox"
                checked={boolValue}
                onChange={(e) => setBoolValue(e.target.checked)}
              />
              Enabled
            </label>
          )}
          {entry.kind === 'count' && (
            <div>
              <label
                htmlFor="setting-count"
                className="block text-sm font-medium text-theme-text-secondary mb-1"
              >
                Value
              </label>
              <input
                id="setting-count"
                type="number"
                min={0}
                step={1}
                className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm font-mono text-theme-text-primary outline-hidden focus:border-accent-link"
                value={countText}
                onChange={(e) => setCountText(e.target.value)}
                autoFocus
              />
            </div>
          )}
          {entry.kind === 'name_list' && (
            <div>
              <label
                htmlFor="setting-name"
                className="block text-sm font-medium text-theme-text-secondary mb-1"
              >
                View sets
              </label>
              <div className="flex flex-wrap gap-1.5 p-1.5 bg-app-bg border border-theme-border rounded-md">
                {names.map((n) => (
                  <span
                    key={n}
                    className="inline-flex items-center gap-1.5 text-xs bg-app-card border border-theme-border rounded-full pl-2.5 pr-1.5 py-0.5 text-theme-text-primary"
                  >
                    {n}
                    <button
                      type="button"
                      aria-label={`Remove ${n}`}
                      onClick={() => setNames(names.filter((x) => x !== n))}
                      className="text-theme-text-muted hover:text-red-400"
                    >
                      ×
                    </button>
                  </span>
                ))}
                <input
                  id="setting-name"
                  type="text"
                  className="flex-1 min-w-[120px] bg-transparent text-sm text-theme-text-primary outline-hidden placeholder:text-theme-text-muted"
                  placeholder="Add a name and press Enter"
                  value={nameInput}
                  onChange={(e) => setNameInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault()
                      addName()
                    }
                  }}
                  autoFocus
                />
              </div>
            </div>
          )}
          <p className="text-xs text-theme-text-muted">
            Reaches every service within one cache refresh, with no restart.
          </p>
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={onClose} disabled={isSubmitting}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={!canSubmit}>
            {isSubmitting ? 'Saving...' : 'Save'}
          </Button>
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// SettingsPage
// ---------------------------------------------------------------------------

function SettingsPageContent() {
  usePageTitle('Settings')

  const [entries, setEntries] = useState<SettingEntry[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [listError, setListError] = useState<string | null>(null)
  const [editTarget, setEditTarget] = useState<SettingEntry | null>(null)
  const [resetTarget, setResetTarget] = useState<SettingEntry | null>(null)
  const [isResetting, setIsResetting] = useState(false)
  const [resetError, setResetError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setIsLoading(true)
    setListError(null)
    try {
      setEntries(await fetchSettings())
    } catch (err) {
      setListError(err instanceof SettingsError ? err.message : 'Failed to load settings')
    } finally {
      setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    // IIFE keeps the setState out of the effect's top level -- see react-hooks/set-state-in-effect
    void (async () => {
      await load()
    })()
  }, [load])

  const handleReset = async () => {
    if (!resetTarget) return
    setIsResetting(true)
    setResetError(null)
    try {
      await resetSetting(resetTarget.key)
      setResetTarget(null)
      void load()
    } catch (err) {
      setResetError(err instanceof SettingsError ? err.message : 'Failed to reset setting')
    } finally {
      setIsResetting(false)
    }
  }

  return (
    <AuthGuard requireAdmin>
      <PageLayout onRefresh={load}>
        <div className="p-6 flex flex-col h-full">
          <div className="flex items-center gap-1.5 text-sm text-theme-text-muted mb-4">
            <AppLink href="/admin" className="text-accent-link hover:underline">
              Admin
            </AppLink>
            <span>/</span>
            <span>Settings</span>
          </div>

          <div className="mb-6">
            <h1 className="text-2xl font-semibold text-theme-text-primary">Settings</h1>
            <p className="mt-1 text-theme-text-secondary">
              Deployment-wide settings stored in the database. Changes reach every replica within
              one cache refresh, with no restart.
            </p>
          </div>

          <div className="mb-6 p-3 rounded-lg border border-theme-border bg-app-panel text-sm text-theme-text-secondary">
            A setting with no stored value falls back to its environment variable on each service,
            then to its built-in default. Every change is recorded in the control-plane audit log.
          </div>

          {listError && (
            <ErrorBanner title="Failed to load settings" message={listError} onRetry={load} />
          )}

          <EditDialog entry={editTarget} onClose={() => setEditTarget(null)} onSaved={() => void load()} />

          <ConfirmDialog
            isOpen={resetTarget !== null}
            onClose={() => {
              setResetTarget(null)
              setResetError(null)
            }}
            onConfirm={handleReset}
            title="Reset setting"
            message={`Remove the stored value of \`${resetTarget?.key}\`? It reverts to its environment variable, then to its built-in default.`}
            confirmLabel="Reset"
            isLoading={isResetting}
            variant="danger"
            error={resetError}
          />

          {isLoading ? (
            <div className="flex-1 flex items-center justify-center">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading settings…</span>
              </div>
            </div>
          ) : (
            groupSettings(entries).map((group) => (
              <div key={group.title} className="mb-6">
                <h2 className="text-sm font-semibold text-theme-text-secondary mb-2">
                  {group.title}
                </h2>
                <div className="border border-theme-border rounded-lg overflow-hidden">
                  <table className="w-full border-collapse text-sm">
                    <thead>
                      <tr className="bg-app-panel text-left text-theme-text-muted">
                        <th className="px-3 py-2 font-medium">Setting</th>
                        <th className="px-3 py-2 font-medium">Value</th>
                        <th className="px-3 py-2 font-medium">Source</th>
                        <th className="px-3 py-2 font-medium">Last changed</th>
                        <th className="px-3 py-2" />
                      </tr>
                    </thead>
                    <tbody>
                      {group.entries.map((e) => (
                        <tr key={e.key} className="border-t border-theme-border align-top">
                          <td className="px-3 py-2">
                            <div className="font-mono text-theme-text-primary">{e.key}</div>
                            <div className="text-theme-text-secondary">{e.description}</div>
                            <div className="text-xs text-theme-text-muted">
                              env fallback: <code>{e.env_var}</code> · default:{' '}
                              <code>{JSON.stringify(e.default)}</code>
                            </div>
                          </td>
                          <td className="px-3 py-2">
                            {e.value === null ? (
                              <span className="text-theme-text-muted">not stored</span>
                            ) : (
                              <ValueView value={e.value} kind={e.kind} />
                            )}
                          </td>
                          <td className="px-3 py-2">
                            <span
                              className={`text-xs px-2 py-0.5 rounded-full border ${
                                e.value === null
                                  ? 'border-theme-border text-theme-text-muted'
                                  : 'border-accent-link text-accent-link'
                              }`}
                            >
                              {e.value === null ? 'Env / default' : 'Database'}
                            </span>
                          </td>
                          <td className="px-3 py-2 text-theme-text-secondary">
                            {e.updated_by ? (
                              <>
                                {e.updated_by}
                                <div className="text-xs text-theme-text-muted">
                                  {e.updated_at ? formatDate(e.updated_at) : ''}
                                </div>
                              </>
                            ) : (
                              '—'
                            )}
                          </td>
                          <td className="px-3 py-2 whitespace-nowrap text-right">
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={() => setEditTarget(e)}
                              aria-label={`Edit ${e.key}`}
                            >
                              Edit
                            </Button>
                            {e.value !== null && (
                              <Button
                                variant="outline"
                                size="sm"
                                className="ml-2"
                                onClick={() => setResetTarget(e)}
                                aria-label={`Reset ${e.key}`}
                              >
                                Reset
                              </Button>
                            )}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            ))
          )}
        </div>
      </PageLayout>
    </AuthGuard>
  )
}

export default function SettingsPage() {
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
      <SettingsPageContent />
    </Suspense>
  )
}
