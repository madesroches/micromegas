import { Suspense, useState, useEffect, useCallback, useMemo, useRef } from 'react'
import { useNavigate, useSearchParams } from 'react-router'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Plus, MoreVertical, Folder, FolderPlus } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { ErrorBanner } from '@/components/ErrorBanner'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { AppLink } from '@/components/AppLink'
import { renderIcon } from '@/lib/screen-type-utils'
import { FolderBreadcrumb } from '@/components/FolderBreadcrumb'
import { FolderPickerModal } from '@/components/FolderPickerModal'
import { normalizeFolderSegment, parentOf } from '@/components/FolderTree'
import {
  listScreens,
  getScreenTypes,
  Screen,
  ScreenTypeInfo,
  ScreenTypeName,
  deleteScreen,
  ScreenApiError,
} from '@/lib/screens-api'
import { listFolders, createFolder, screenMatchesQuery, FolderInfo } from '@/lib/folders-api'
import { notifyFoldersChanged, useFoldersChangedListener } from '@/lib/folders-sync'
import { useMoveScreen } from '@/hooks/useMoveScreen'

function ScreensPageContent() {
  usePageTitle('Screens')
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const [screens, setScreens] = useState<Screen[]>([])
  const [screenTypes, setScreenTypes] = useState<ScreenTypeInfo[]>([])
  const [folders, setFolders] = useState<FolderInfo[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [deletingScreen, setDeletingScreen] = useState<string | null>(null)
  const [screenToDelete, setScreenToDelete] = useState<string | null>(null)
  const [openMenuFor, setOpenMenuFor] = useState<string | null>(null)
  const [movingScreen, setMovingScreen] = useState<string | null>(null)
  const [draggingScreen, setDraggingScreen] = useState<string | null>(null)
  const [creatingFolder, setCreatingFolder] = useState(false)
  const [newFolderName, setNewFolderName] = useState('')

  // `?folder=<path>` is the single source of truth for the selected folder;
  // absent means the root (Home) folder. `?q=` drives the search view; both
  // params are also read/written by the persistent sidebar (Sidebar.tsx).
  const selectedFolder = searchParams.get('folder') ?? ''
  const searchQuery = searchParams.get('q') ?? ''
  const isSearching = searchQuery.trim().length > 0

  const loadData = useCallback(async () => {
    setIsLoading(true)
    setError(null)
    try {
      const [screensData, typesData, foldersData] = await Promise.all([
        listScreens(),
        getScreenTypes(),
        listFolders(),
      ])
      setScreens(screensData)
      setScreenTypes(typesData)
      setFolders(foldersData)
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
    // IIFE keeps the setState out of the effect's top level — see react-hooks/set-state-in-effect
    void (async () => {
      await loadData()
    })()
  }, [loadData])

  useFoldersChangedListener(loadData)

  const selectFolder = useCallback(
    (path: string) => {
      const next = new URLSearchParams(searchParams)
      next.set('folder', path)
      next.delete('q')
      setSearchParams(next)
    },
    [searchParams, setSearchParams]
  )

  const handleCreateNew = (typeName: ScreenTypeName) => {
    navigate(`/screen/new?type=${typeName}`)
  }

  const normalizedNewFolderName = useMemo(() => normalizeFolderSegment(newFolderName), [newFolderName])
  // Set by Escape's keydown handler, checked (and reset) at the top of
  // commitCreateFolder. Needed because unmounting the focused <input> on the
  // next render fires a native blur that React replays through the
  // *previous* render's onBlur closure — which still holds the pre-Escape
  // typed value — so the blur-driven commit must be able to see that Escape
  // just happened and bail out instead of saving.
  const createFolderCancelledRef = useRef(false)

  const commitCreateFolder = async () => {
    if (createFolderCancelledRef.current) {
      createFolderCancelledRef.current = false
      return
    }
    if (!normalizedNewFolderName) {
      setCreatingFolder(false)
      setNewFolderName('')
      return
    }
    const path = selectedFolder ? `${selectedFolder}/${normalizedNewFolderName}` : normalizedNewFolderName
    setActionError(null)
    try {
      await createFolder(path)
      notifyFoldersChanged()
      await loadData()
    } catch (err) {
      setActionError(
        err instanceof ScreenApiError ? `Failed to create folder: ${err.message}` : 'Failed to create folder'
      )
    } finally {
      setCreatingFolder(false)
      setNewFolderName('')
    }
  }

  const handleDeleteClick = (screenName: string, e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()
    setOpenMenuFor(null)
    setScreenToDelete(screenName)
  }

  const handleDeleteConfirm = async () => {
    if (!screenToDelete) return

    setDeletingScreen(screenToDelete)
    setActionError(null)
    try {
      await deleteScreen(screenToDelete)
      setScreenToDelete(null)
      notifyFoldersChanged()
      await loadData()
    } catch (err) {
      if (err instanceof ScreenApiError) {
        setActionError(`Failed to delete: ${err.message}`)
      } else {
        setActionError('Failed to delete screen')
      }
      setScreenToDelete(null)
    } finally {
      setDeletingScreen(null)
    }
  }

  const handleMoveScreen = useMoveScreen(screens, loadData, setActionError)

  // Create lookup map for screen type info
  const screenTypeMap = useMemo(() => {
    const map = new Map<ScreenTypeName, ScreenTypeInfo>()
    for (const type of screenTypes) {
      map.set(type.name, type)
    }
    return map
  }, [screenTypes])

  const searchResults = useMemo(() => {
    const q = searchQuery.trim().toLowerCase()
    if (!q) return []
    return screens.filter((s) => screenMatchesQuery(s, q)).sort((a, b) => a.name.localeCompare(b.name))
  }, [screens, searchQuery])

  const directSubfolders = useMemo(
    () => folders.filter((f) => parentOf(f.path) === selectedFolder).sort((a, b) => a.path.localeCompare(b.path)),
    [folders, selectedFolder]
  )

  const directScreens = useMemo(
    () => screens.filter((s) => s.folder_path === selectedFolder).sort((a, b) => a.name.localeCompare(b.name)),
    [screens, selectedFolder]
  )

  const screenCard = (screen: Screen, showPath: boolean) => (
    <div
      key={screen.name}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData('text/plain', screen.name)
        e.dataTransfer.effectAllowed = 'move'
        setDraggingScreen(screen.name)
      }}
      onDragEnd={() => setDraggingScreen(null)}
      className={`group relative p-4 rounded-lg border border-theme-border bg-app-panel hover:bg-app-card hover:border-accent-link transition-colors cursor-grab ${
        draggingScreen === screen.name ? 'opacity-40' : ''
      }`}
    >
      <AppLink href={`/screen/${screen.name}`} className="block">
        <div className="min-w-0 pr-6">
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
          {showPath && (
            <p className="text-xs text-theme-text-muted truncate font-mono mt-0.5">
              /{screen.folder_path}
            </p>
          )}
        </div>
      </AppLink>

      <div className="absolute top-3 right-2">
        <button
          onClick={(e) => {
            e.preventDefault()
            e.stopPropagation()
            setOpenMenuFor(openMenuFor === screen.name ? null : screen.name)
          }}
          className="p-1.5 rounded-sm text-theme-text-muted hover:text-theme-text-primary hover:bg-theme-border opacity-0 group-hover:opacity-100 transition-opacity"
          title="Screen actions"
        >
          <MoreVertical className="w-4 h-4" />
        </button>
        {openMenuFor === screen.name && (
          <div className="absolute right-0 mt-1 w-40 bg-app-card border border-theme-border-hover rounded-md shadow-lg z-10 overflow-hidden">
            <button
              onClick={(e) => {
                e.preventDefault()
                e.stopPropagation()
                setOpenMenuFor(null)
                setMovingScreen(screen.name)
              }}
              className="w-full text-left px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border"
            >
              Move to folder
            </button>
            <button
              onClick={(e) => handleDeleteClick(screen.name, e)}
              disabled={deletingScreen === screen.name}
              className="w-full text-left px-3 py-2 text-sm text-red-400 hover:bg-theme-border"
            >
              Delete
            </button>
          </div>
        )}
      </div>
    </div>
  )

  const folderCard = (folder: FolderInfo) => (
    <div
      key={folder.path}
      role="button"
      tabIndex={0}
      onClick={() => selectFolder(folder.path)}
      onKeyDown={(e) => e.key === 'Enter' && selectFolder(folder.path)}
      onDragOver={(e) => {
        e.preventDefault()
        e.dataTransfer.dropEffect = 'move'
      }}
      onDrop={(e) => {
        e.preventDefault()
        const name = e.dataTransfer.getData('text/plain')
        if (name) handleMoveScreen(name, folder.path)
      }}
      className="p-4 rounded-lg border border-theme-border bg-app-panel hover:bg-app-card hover:border-accent-link transition-colors cursor-pointer"
    >
      <div className="flex items-center gap-2 mb-1">
        <Folder className="w-4 h-4 text-accent-warning" />
        <h3 className="font-medium text-theme-text-primary truncate">{folder.path.split('/').pop()}</h3>
      </div>
      <p className="text-xs text-theme-text-muted">
        {folder.subfolder_count} subfolder{folder.subfolder_count === 1 ? '' : 's'} · {folder.screen_count} screen
        {folder.screen_count === 1 ? '' : 's'}
      </p>
    </div>
  )

  const movingScreenObj = movingScreen ? screens.find((s) => s.name === movingScreen) : null

  return (
    <AuthGuard>
      <PageLayout onRefresh={loadData}>
        <div className="p-6 flex flex-col h-full" onClick={() => setOpenMenuFor(null)}>
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
          {actionError && (
            <div className="mb-4">
              <ErrorBanner title="Action failed" message={actionError} />
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
            <div className="flex-1 overflow-hidden flex flex-col">
              {/* Create New Buttons */}
              <div className="flex flex-wrap gap-2 mb-4">
                {screenTypes.map((type) => (
                  <Button
                    key={type.name}
                    variant="outline"
                    size="sm"
                    onClick={() => handleCreateNew(type.name)}
                    className="gap-1.5"
                  >
                    <Plus className="w-4 h-4" />
                    {renderIcon(type.icon)}
                    New {type.display_name}
                  </Button>
                ))}
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    setNewFolderName('')
                    setCreatingFolder(true)
                  }}
                  disabled={isSearching}
                  className="gap-1.5"
                >
                  <FolderPlus className="w-4 h-4" />
                  New Folder
                </Button>
              </div>

              {creatingFolder && (
                <div className="mb-4 max-w-xs">
                  <input
                    autoFocus
                    value={newFolderName}
                    onChange={(e) => setNewFolderName(e.target.value)}
                    onBlur={commitCreateFolder}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') commitCreateFolder()
                      if (e.key === 'Escape') {
                        createFolderCancelledRef.current = true
                        setCreatingFolder(false)
                        setNewFolderName('')
                      }
                    }}
                    placeholder="Folder name"
                    className="w-full px-2 py-1 text-sm bg-app-bg border border-accent-link rounded-sm text-theme-text-primary outline-hidden"
                  />
                  {normalizedNewFolderName && normalizedNewFolderName !== newFolderName.toLowerCase() && (
                    <p className="text-[11px] text-theme-text-muted mt-0.5">
                      Will be saved as: <span className="font-mono text-accent-link">{normalizedNewFolderName}</span>
                    </p>
                  )}
                </div>
              )}

              {/* Content */}
              <div className="flex-1 overflow-auto">
                {isSearching ? (
                  <>
                    <FolderBreadcrumb path={null} onNavigate={selectFolder} />
                    <p className="text-xs text-theme-text-muted mb-3">
                      {searchResults.length} match{searchResults.length === 1 ? '' : 'es'} across all folders
                    </p>
                    {searchResults.length > 0 ? (
                      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
                        {searchResults.map((s) => screenCard(s, true))}
                      </div>
                    ) : (
                      <div className="p-8 rounded-lg border border-dashed border-theme-border text-center">
                        <p className="text-theme-text-muted">No screens or folders match &ldquo;{searchQuery}&rdquo;.</p>
                      </div>
                    )}
                  </>
                ) : (
                  <>
                    <FolderBreadcrumb path={selectedFolder} onNavigate={selectFolder} onDropScreen={handleMoveScreen} />
                    {directSubfolders.length > 0 || directScreens.length > 0 ? (
                      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
                        {directSubfolders.map((f) => folderCard(f))}
                        {directScreens.map((s) => screenCard(s, false))}
                      </div>
                    ) : (
                      <div className="p-8 rounded-lg border border-dashed border-theme-border text-center">
                        <p className="text-theme-text-muted">This folder is empty. Create a screen or a subfolder to get started.</p>
                      </div>
                    )}
                  </>
                )}
              </div>
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

        {/* Move-to-folder Modal */}
        <FolderPickerModal
          isOpen={movingScreen !== null}
          onClose={() => setMovingScreen(null)}
          onSelect={(path) => {
            if (movingScreen) handleMoveScreen(movingScreen, path)
          }}
          currentPath={movingScreenObj?.folder_path}
          folders={folders}
          title={movingScreen ? `Move "${movingScreen}"` : 'Move to folder'}
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
