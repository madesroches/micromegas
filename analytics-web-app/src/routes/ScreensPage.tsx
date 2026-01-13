import { Suspense, useState, useEffect, useCallback } from 'react'
import { useNavigate } from 'react-router-dom'
import { List, LineChart, FileText, Plus, Trash2 } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { ErrorBanner } from '@/components/ErrorBanner'
import { Button } from '@/components/ui/button'
import { AppLink } from '@/components/AppLink'
import {
  listScreens,
  getScreenTypes,
  Screen,
  ScreenTypeInfo,
  ScreenTypeName,
  deleteScreen,
  ScreenApiError,
} from '@/lib/screens-api'

// Map screen type names to icons
function getScreenTypeIcon(typeName: ScreenTypeName) {
  switch (typeName) {
    case 'process_list':
      return <List className="w-5 h-5" />
    case 'metrics':
      return <LineChart className="w-5 h-5" />
    case 'log':
      return <FileText className="w-5 h-5" />
    default:
      return <FileText className="w-5 h-5" />
  }
}

// Get display name for screen type
function getScreenTypeDisplayName(typeName: ScreenTypeName): string {
  switch (typeName) {
    case 'process_list':
      return 'Process List'
    case 'metrics':
      return 'Metrics'
    case 'log':
      return 'Log'
    default:
      return typeName
  }
}

function ScreensPageContent() {
  const navigate = useNavigate()
  const [screens, setScreens] = useState<Screen[]>([])
  const [screenTypes, setScreenTypes] = useState<ScreenTypeInfo[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [deleteError, setDeleteError] = useState<string | null>(null)
  const [deletingScreen, setDeletingScreen] = useState<string | null>(null)

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

  const handleDelete = async (screenName: string, e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()

    if (!confirm(`Delete screen "${screenName}"?`)) {
      return
    }

    setDeletingScreen(screenName)
    setDeleteError(null)
    try {
      await deleteScreen(screenName)
      setScreens((prev) => prev.filter((s) => s.name !== screenName))
    } catch (err) {
      if (err instanceof ScreenApiError) {
        setDeleteError(`Failed to delete: ${err.message}`)
      } else {
        setDeleteError('Failed to delete screen')
      }
    } finally {
      setDeletingScreen(null)
    }
  }

  // Group screens by type
  const screensByType = screens.reduce(
    (acc, screen) => {
      const type = screen.screen_type
      if (!acc[type]) {
        acc[type] = []
      }
      acc[type].push(screen)
      return acc
    },
    {} as Record<ScreenTypeName, Screen[]>
  )

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
              {/* Screen Types Grid */}
              {screenTypes.map((type) => {
                const typeScreens = screensByType[type.name] || []

                return (
                  <div key={type.name} className="mb-8">
                    {/* Type Header */}
                    <div className="flex items-center justify-between mb-4">
                      <div className="flex items-center gap-3">
                        <div className="p-2 rounded-md bg-app-card text-accent-link">
                          {getScreenTypeIcon(type.name)}
                        </div>
                        <div>
                          <h2 className="text-lg font-medium text-theme-text-primary">
                            {getScreenTypeDisplayName(type.name)}
                          </h2>
                          <p className="text-sm text-theme-text-secondary">{type.description}</p>
                        </div>
                      </div>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => handleCreateNew(type.name)}
                        className="gap-1"
                      >
                        <Plus className="w-4 h-4" />
                        Create New
                      </Button>
                    </div>

                    {/* Screens List */}
                    {typeScreens.length > 0 ? (
                      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
                        {typeScreens.map((screen) => (
                          <AppLink
                            key={screen.name}
                            href={`/screen/${screen.name}`}
                            className="group block"
                          >
                            <div className="p-4 rounded-lg border border-theme-border bg-app-panel hover:bg-app-card hover:border-accent-link transition-colors">
                              <div className="flex items-start justify-between">
                                <div className="min-w-0 flex-1">
                                  <h3 className="font-medium text-theme-text-primary truncate group-hover:text-accent-link transition-colors">
                                    {screen.name}
                                  </h3>
                                  <p className="mt-1 text-xs text-theme-text-muted truncate">
                                    Updated{' '}
                                    {new Date(screen.updated_at).toLocaleDateString(undefined, {
                                      month: 'short',
                                      day: 'numeric',
                                      year: 'numeric',
                                    })}
                                  </p>
                                </div>
                                <button
                                  onClick={(e) => handleDelete(screen.name, e)}
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
                      <div className="p-6 rounded-lg border border-dashed border-theme-border text-center">
                        <p className="text-theme-text-muted">
                          No {getScreenTypeDisplayName(type.name).toLowerCase()} screens yet.
                        </p>
                        <Button
                          variant="link"
                          size="sm"
                          onClick={() => handleCreateNew(type.name)}
                          className="mt-2"
                        >
                          Create your first one
                        </Button>
                      </div>
                    )}
                  </div>
                )
              })}
            </div>
          )}
        </div>
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
