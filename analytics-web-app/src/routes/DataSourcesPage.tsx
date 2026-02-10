import { Suspense, useState, useEffect, useCallback } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Plus, Star, Trash2, Pencil } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { Button } from '@/components/ui/button'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import {
  listDataSources,
  getDataSource,
  createDataSource,
  updateDataSource,
  deleteDataSource,
  DataSourceSummary,
  DataSourceApiError,
} from '@/lib/data-sources-api'

interface FormState {
  mode: 'create' | 'edit'
  name: string
  url: string
  isDefault: boolean
  originalName?: string
  saving: boolean
  error: string | null
}

const initialFormState: FormState = {
  mode: 'create',
  name: '',
  url: '',
  isDefault: false,
  saving: false,
  error: null,
}

function DataSourcesPageContent() {
  usePageTitle('Data Sources')

  const [dataSources, setDataSources] = useState<DataSourceSummary[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [form, setForm] = useState<FormState | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)
  const [isDeleting, setIsDeleting] = useState(false)

  const loadData = useCallback(async () => {
    setIsLoading(true)
    setError(null)
    try {
      const data = await listDataSources()
      setDataSources(data)
    } catch (err) {
      setError(err instanceof DataSourceApiError ? err.message : 'Failed to load data sources')
    } finally {
      setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    loadData()
  }, [loadData])

  const openCreate = () => {
    setForm({ ...initialFormState, mode: 'create' })
  }

  const openEdit = async (name: string) => {
    try {
      const ds = await getDataSource(name)
      setForm({
        mode: 'edit',
        name: ds.name,
        url: ds.config.url,
        isDefault: ds.is_default,
        originalName: ds.name,
        saving: false,
        error: null,
      })
    } catch (err) {
      setError(err instanceof DataSourceApiError ? err.message : 'Failed to load data source')
    }
  }

  const handleSave = async () => {
    if (!form) return

    setForm((f) => (f ? { ...f, saving: true, error: null } : null))

    try {
      if (form.mode === 'create') {
        await createDataSource({
          name: form.name,
          config: { url: form.url },
          is_default: form.isDefault,
        })
      } else if (form.originalName) {
        await updateDataSource(form.originalName, {
          config: { url: form.url },
          is_default: form.isDefault || undefined,
        })
      }
      setForm(null)
      await loadData()
    } catch (err) {
      const message = err instanceof DataSourceApiError ? err.message : 'Failed to save'
      setForm((f) => (f ? { ...f, saving: false, error: message } : null))
    }
  }

  const handleDelete = async () => {
    if (!deleteTarget) return
    setIsDeleting(true)
    try {
      await deleteDataSource(deleteTarget)
      setDeleteTarget(null)
      await loadData()
    } catch (err) {
      setError(err instanceof DataSourceApiError ? err.message : 'Failed to delete data source')
      setDeleteTarget(null)
    } finally {
      setIsDeleting(false)
    }
  }

  const handleSetDefault = async (name: string) => {
    try {
      await updateDataSource(name, { is_default: true })
      await loadData()
    } catch (err) {
      setError(err instanceof DataSourceApiError ? err.message : 'Failed to set default')
    }
  }

  return (
    <AuthGuard>
      <PageLayout onRefresh={loadData}>
        <div className="p-6 flex flex-col h-full">
          {/* Breadcrumb */}
          <div className="flex items-center gap-1.5 text-sm text-theme-text-muted mb-4">
            <AppLink href="/admin" className="text-accent-link hover:underline">
              Admin
            </AppLink>
            <span>/</span>
            <span>Data Sources</span>
          </div>

          <div className="flex items-center justify-between mb-6">
            <div>
              <h1 className="text-2xl font-semibold text-theme-text-primary">Data Sources</h1>
              <p className="mt-1 text-theme-text-secondary">
                Manage FlightSQL server connections for queries.
              </p>
            </div>
            <Button onClick={openCreate} className="gap-1.5">
              <Plus className="w-4 h-4" />
              Add Data Source
            </Button>
          </div>

          {error && (
            <ErrorBanner title="Error" message={error} onDismiss={() => setError(null)} />
          )}

          {/* Form dialog */}
          {form && (
            <div className="fixed inset-0 z-50 flex items-center justify-center">
              <div className="absolute inset-0 bg-black/50" onClick={() => !form.saving && setForm(null)} />
              <div className="relative w-full max-w-md bg-app-panel border border-theme-border rounded-lg shadow-xl">
                <div className="px-4 py-3 border-b border-theme-border">
                  <h2 className="text-lg font-medium text-theme-text-primary">
                    {form.mode === 'create' ? 'Add Data Source' : 'Edit Data Source'}
                  </h2>
                </div>
                <div className="p-4 space-y-4">
                  {form.error && (
                    <div className="p-3 bg-accent-error/10 border border-accent-error/30 rounded-lg text-sm text-accent-error">
                      {form.error}
                    </div>
                  )}
                  <div>
                    <label className="block text-sm font-medium text-theme-text-secondary mb-1">
                      Name
                    </label>
                    <input
                      type="text"
                      className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm text-theme-text-primary placeholder:text-theme-text-muted outline-none focus:border-accent-link disabled:opacity-50"
                      placeholder="e.g. production"
                      value={form.name}
                      onChange={(e) => setForm((f) => (f ? { ...f, name: e.target.value } : null))}
                      disabled={form.mode === 'edit'}
                    />
                  </div>
                  <div>
                    <label className="block text-sm font-medium text-theme-text-secondary mb-1">
                      FlightSQL URL
                    </label>
                    <input
                      type="text"
                      className="w-full bg-app-bg border border-theme-border rounded-md px-3 py-2 text-sm text-theme-text-primary placeholder:text-theme-text-muted outline-none focus:border-accent-link"
                      placeholder="https://flight-sql.example.com:443"
                      value={form.url}
                      onChange={(e) => setForm((f) => (f ? { ...f, url: e.target.value } : null))}
                    />
                  </div>
                  <label className="flex items-center gap-2 text-sm text-theme-text-secondary cursor-pointer">
                    <input
                      type="checkbox"
                      className="accent-[var(--color-accent-link)]"
                      checked={form.isDefault}
                      onChange={(e) =>
                        setForm((f) => (f ? { ...f, isDefault: e.target.checked } : null))
                      }
                    />
                    Set as default data source
                  </label>
                </div>
                <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
                  <Button variant="outline" onClick={() => setForm(null)} disabled={form.saving}>
                    Cancel
                  </Button>
                  <Button
                    onClick={handleSave}
                    disabled={form.saving || !form.name.trim() || !form.url.trim()}
                  >
                    {form.saving ? (
                      <span className="flex items-center gap-2">
                        <span className="w-4 h-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                        Saving...
                      </span>
                    ) : form.mode === 'create' ? (
                      'Create'
                    ) : (
                      'Save'
                    )}
                  </Button>
                </div>
              </div>
            </div>
          )}

          {/* Delete confirm dialog */}
          <ConfirmDialog
            isOpen={deleteTarget !== null}
            onClose={() => setDeleteTarget(null)}
            onConfirm={handleDelete}
            title="Delete Data Source"
            message={`Are you sure you want to delete "${deleteTarget}"? This action cannot be undone.`}
            confirmLabel="Delete"
            isLoading={isDeleting}
            variant="danger"
          />

          {isLoading ? (
            <div className="flex-1 flex items-center justify-center">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading data sources...</span>
              </div>
            </div>
          ) : dataSources.length === 0 ? (
            <div className="flex-1 flex flex-col items-center justify-center text-center">
              <p className="text-theme-text-muted mb-4">
                No data sources configured yet. Add one to get started.
              </p>
              <Button onClick={openCreate} className="gap-1.5">
                <Plus className="w-4 h-4" />
                Add Data Source
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
                      Default
                    </th>
                    <th className="text-right p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Actions
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {dataSources.map((ds) => (
                    <tr
                      key={ds.name}
                      className="border-t border-theme-border hover:bg-accent-link/5"
                    >
                      <td className="p-2.5 px-4">
                        <span className="text-accent-link font-medium">{ds.name}</span>
                      </td>
                      <td className="p-2.5 px-4">
                        {ds.is_default ? (
                          <span className="inline-flex items-center gap-1 px-2 py-0.5 bg-yellow-500/15 text-yellow-500 rounded text-xs font-medium">
                            <Star className="w-3 h-3" />
                            Default
                          </span>
                        ) : (
                          <button
                            onClick={() => handleSetDefault(ds.name)}
                            className="text-xs text-theme-text-muted hover:text-accent-link transition-colors"
                          >
                            Set as default
                          </button>
                        )}
                      </td>
                      <td className="p-2.5 px-4 text-right">
                        <div className="flex items-center justify-end gap-1">
                          <button
                            onClick={() => openEdit(ds.name)}
                            className="p-1.5 rounded text-theme-text-muted hover:text-accent-link hover:bg-accent-link/10 transition-colors"
                            title="Edit"
                          >
                            <Pencil className="w-4 h-4" />
                          </button>
                          {!ds.is_default && (
                            <button
                              onClick={() => setDeleteTarget(ds.name)}
                              className="p-1.5 rounded text-theme-text-muted hover:text-red-400 hover:bg-red-400/10 transition-colors"
                              title="Delete"
                            >
                              <Trash2 className="w-4 h-4" />
                            </button>
                          )}
                        </div>
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

export default function DataSourcesPage() {
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
      <DataSourcesPageContent />
    </Suspense>
  )
}
