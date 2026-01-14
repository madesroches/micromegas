import { Suspense, useState, useEffect, useCallback, useMemo } from 'react'
import { useNavigate } from 'react-router-dom'
import { Plus, Trash2 } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { ErrorBanner } from '@/components/ErrorBanner'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { AppLink } from '@/components/AppLink'
import { renderIcon } from '@/lib/screen-type-utils'
import {
  listScreens,
  getScreenTypes,
  Screen,
  ScreenTypeInfo,
  ScreenTypeName,
  deleteScreen,
  ScreenApiError,
} from '@/lib/screens-api'

function ScreensPageContent() {
  const navigate = useNavigate()
  const [screens, setScreens] = useState<Screen[]>([])
  const [screenTypes, setScreenTypes] = useState<ScreenTypeInfo[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [deleteError, setDeleteError] = useState<string | null>(null)
  const [deletingScreen, setDeletingScreen] = useState<string | null>(null)
  const [screenToDelete, setScreenToDelete] = useState<string | null>(null)

  const loadData = useCallback(async () => {
    setIsLoading(true)
    setError(null)
    try {
      const [screensData, typesData] = await Promise.all([
        listScreens(),
        getScreenTypes(),
      ])
      setScreens(screensData)
      setScreenTypes(typesData)
    } catch (err) {
      if (err instanceof ScreenApiError) {
        setError(err.message)
      } else {
        setError('Failed to load screens')
      }
    } finally {
      setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    loadData()
  }, [loadData])

  const handleCreateNew = (typeName: ScreenTypeName) => {
    navigate(`/screen/new?type=${typeName}`)
  }

  const handleDeleteClick = (screenName: string, e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()
    setScreenToDelete(screenName)
  }

  const handleDeleteConfirm = async () => {
    if (!screenToDelete) return

    setDeletingScreen(screenToDelete)
    setDeleteError(null)
    try {
      await deleteScreen(screenToDelete)
      setScreens((prev) => prev.filter((s) => s.name !== screenToDelete))
      setScreenToDelete(null)
    } catch (err) {
      if (err instanceof ScreenApiError) {
        setDeleteError(`Failed to delete: ${err.message}`)
      } else {
        setDeleteError('Failed to delete screen')
      }
      setScreenToDelete(null)
    } finally {
      setDeletingScreen(null)
    }
  }

  // Create lookup map for screen type info
  const screenTypeMap = useMemo(() => {
    const map = new Map<ScreenTypeName, ScreenTypeInfo>()
    for (const type of screenTypes) {
      map.set(type.name, type)
    }
    return map
  }, [screenTypes])

  return (
    <AuthGuard>
      <PageLayout onRefresh={loadData}>
        <div className="p-6 flex flex-col h-full">
          {/* Page Header */}
          <div className="mb-6">
            <h1 className="text-2xl font-semibold text-theme-text-primary">Screens</h1>
            <p className="mt-1 text-theme-text-secondary">
              Create and manage custom screens with editable SQL queries.
            </p>
          </div>

          {/* Error Banners */}
          {error && (
            <ErrorBanner title="Failed to load screens" message={error} onRetry={loadData} />
          )}
          {deleteError && (
            <div className="mb-4">
              <ErrorBanner title="Delete failed" message={deleteError} />
            </div>
          )}

          {/* Loading State */}
          {isLoading ? (
            <div className="flex-1 flex items-center justify-center">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading screens...</span>
              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-auto">
              {/* Create New Buttons */}
              <div className="flex flex-wrap gap-2 mb-6">
                {screenTypes.map((type) => (
                  <Button
                    key={type.name}
                    variant="outline"
                    size="sm"
                    onClick={() => handleCreateNew(type.name)}
                    className="gap-1.5"
                  >
                    <Plus className="w-4 h-4" />
                    New {type.display_name}
                  </Button>
                ))}
              </div>

              {/* Flat Screens List */}
              {screens.length > 0 ? (
                <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
                  {screens.map((screen) => (
                    <AppLink
                      key={screen.name}
                      href={`/screen/${screen.name}`}
                      className="group block"
                    >
                      <div className="p-4 rounded-lg border border-theme-border bg-app-panel hover:bg-app-card hover:border-accent-link transition-colors">
                        <div className="flex items-start justify-between">
                          <div className="min-w-0 flex-1">
                            <div className="flex items-center gap-2 mb-1">
                              <span className="text-accent-link">
                                {renderIcon(screenTypeMap.get(screen.screen_type)?.icon ?? 'file-text')}
                              </span>
                              <h3 className="font-medium text-theme-text-primary truncate group-hover:text-accent-link transition-colors">
                                {screen.name}
                              </h3>
                            </div>
                            <p className="text-xs text-theme-text-muted truncate">
                              {screenTypeMap.get(screen.screen_type)?.display_name ?? screen.screen_type} · Updated{' '}
                              {new Date(screen.updated_at).toLocaleDateString(undefined, {
                                month: 'short',
                                day: 'numeric',
                                year: 'numeric',
                              })}
                            </p>
                          </div>
                          <button
                            onClick={(e) => handleDeleteClick(screen.name, e)}
                            disabled={deletingScreen === screen.name}
                            className="ml-2 p-1.5 rounded text-theme-text-muted hover:text-red-400 hover:bg-red-400/10 transition-colors opacity-0 group-hover:opacity-100"
                            title="Delete screen"
                          >
                            {deletingScreen === screen.name ? (
                              <div className="w-4 h-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                            ) : (
                              <Trash2 className="w-4 h-4" />
                            )}
                          </button>
                        </div>
                      </div>
                    </AppLink>
                  ))}
                </div>
              ) : (
                <div className="p-8 rounded-lg border border-dashed border-theme-border text-center">
                  <p className="text-theme-text-muted mb-2">No screens yet.</p>
                  <p className="text-sm text-theme-text-muted">
                    Create your first screen using the buttons above.
                  </p>
                </div>
              )}
            </div>
          )}
        </div>

        {/* Delete Confirmation Dialog */}
        <ConfirmDialog
          isOpen={screenToDelete !== null}
          onClose={() => setScreenToDelete(null)}
          onConfirm={handleDeleteConfirm}
          title="Delete Screen"
          message={`Are you sure you want to delete "${screenToDelete}"? This action cannot be undone.`}
          confirmLabel="Delete"
          variant="danger"
          isLoading={deletingScreen !== null}
        />
      </PageLayout>
    </AuthGuard>
  )
}

export default function ScreensPage() {
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
      <ScreensPageContent />
    </Suspense>
  )
}
