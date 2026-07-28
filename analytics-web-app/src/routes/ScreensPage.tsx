import { Suspense, useState, useEffect, useCallback, useMemo } from 'react'
import { useNavigate, useSearchParams } from 'react-router'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Plus, MoreVertical, Search, X, Folder } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { ErrorBanner } from '@/components/ErrorBanner'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { AppLink } from '@/components/AppLink'
import { renderIcon } from '@/lib/screen-type-utils'
import { FolderTree, ancestorPaths } from '@/components/FolderTree'
import { FolderBreadcrumb } from '@/components/FolderBreadcrumb'
import { FolderPickerModal } from '@/components/FolderPickerModal'
import {
  listScreens,
  getScreenTypes,
  updateScreen,
  Screen,
  ScreenTypeInfo,
  ScreenTypeName,
  deleteScreen,
  ScreenApiError,
} from '@/lib/screens-api'
import { listFolders, createFolder, moveFolder, deleteFolder, FolderInfo } from '@/lib/folders-api'

function parentPath(path: string): string {
  const idx = path.lastIndexOf('/')
  return idx === -1 ? '' : path.slice(0, idx)
}

function computeMatchedFolders(screens: Screen[], query: string): Set<string> {
  const set = new Set<string>()
  const q = query.trim().toLowerCase()
  if (!q) return set
  for (const s of screens) {
    if (s.name.toLowerCase().includes(q) || s.folder_path.toLowerCase().includes(q)) {
      let cur = ''
      for (const part of s.folder_path.split('/')) {
        if (!part) continue
        cur = cur ? `${cur}/${part}` : part
        set.add(cur)
      }
    }
  }
  return set
}

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
  const [searchQuery, setSearchQuery] = useState('')
  const [expandedPaths, setExpandedPaths] = useState<Set<string>>(new Set())
  const [openMenuFor, setOpenMenuFor] = useState<string | null>(null)
  const [movingScreen, setMovingScreen] = useState<string | null>(null)
  const [draggingScreen, setDraggingScreen] = useState<string | null>(null)

  // `?folder=<path>` is the single source of truth for the selected folder;
  // absent means the "All Screens" view.
  const selectedFolder = searchParams.get('folder')
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
    loadData()
  }, [loadData])

  // Keep the tree expanded down to whatever folder the URL points at.
  useEffect(() => {
    if (selectedFolder) {
      setExpandedPaths((prev) => {
        const next = new Set(prev)
        ancestorPaths(selectedFolder).forEach((p) => next.add(p))
        next.add(selectedFolder)
        return next
      })
    }
  }, [selectedFolder])

  const selectFolder = useCallback(
    (path: string) => {
      setSearchQuery('')
      const next = new URLSearchParams(searchParams)
      next.set('folder', path)
      setSearchParams(next)
    },
    [searchParams, setSearchParams]
  )

  const selectAll = useCallback(() => {
    setSearchQuery('')
    const next = new URLSearchParams(searchParams)
    next.delete('folder')
    setSearchParams(next)
  }, [searchParams, setSearchParams])

  const toggleExpand = useCallback((path: string) => {
    setExpandedPaths((prev) => {
      const next = new Set(prev)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  }, [])

  const handleCreateNew = (typeName: ScreenTypeName) => {
    navigate(`/screen/new?type=${typeName}`)
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

  const handleMoveScreen = useCallback(
    async (screenName: string, destPath: string) => {
      const screen = screens.find((s) => s.name === screenName)
      if (!screen || screen.folder_path === destPath) return
      setActionError(null)
      try {
        await updateScreen(screenName, { folder_path: destPath })
        await loadData()
      } catch (err) {
        setActionError(err instanceof ScreenApiError ? `Failed to move: ${err.message}` : 'Failed to move screen')
      }
    },
    [screens, loadData]
  )

  const handleCreateFolder = useCallback(
    async (parent: string, name: string) => {
      const path = parent ? `${parent}/${name}` : name
      setActionError(null)
      try {
        await createFolder(path)
        await loadData()
      } catch (err) {
        setActionError(
          err instanceof ScreenApiError ? `Failed to create folder: ${err.message}` : 'Failed to create folder'
        )
      }
    },
    [loadData]
  )

  const handleRenameFolder = useCallback(
    async (path: string, newPath: string) => {
      setActionError(null)
      try {
        await moveFolder(path, newPath)
        // Keep the current view pointed at the renamed folder (or a
        // descendant of it) instead of a now-nonexistent path.
        if (selectedFolder === path) {
          selectFolder(newPath)
        } else if (selectedFolder && selectedFolder.startsWith(`${path}/`)) {
          selectFolder(`${newPath}${selectedFolder.slice(path.length)}`)
        }
        await loadData()
      } catch (err) {
        setActionError(
          err instanceof ScreenApiError ? `Failed to rename folder: ${err.message}` : 'Failed to rename folder'
        )
      }
    },
    [loadData, selectedFolder, selectFolder]
  )

  const handleDeleteFolder = useCallback(
    async (path: string) => {
      setActionError(null)
      try {
        await deleteFolder(path)
        if (selectedFolder === path) {
          selectFolder(parentPath(path))
        }
        await loadData()
      } catch (err) {
        setActionError(
          err instanceof ScreenApiError ? `Failed to delete folder: ${err.message}` : 'Failed to delete folder'
        )
      }
    },
    [loadData, selectedFolder, selectFolder]
  )

  // Create lookup map for screen type info
  const screenTypeMap = useMemo(() => {
    const map = new Map<ScreenTypeName, ScreenTypeInfo>()
    for (const type of screenTypes) {
      map.set(type.name, type)
    }
    return map
  }, [screenTypes])

  const matchedFolders = useMemo(() => computeMatchedFolders(screens, searchQuery), [screens, searchQuery])

  const searchResults = useMemo(() => {
    const q = searchQuery.trim().toLowerCase()
    if (!q) return []
    return screens
      .filter((s) => s.name.toLowerCase().includes(q) || s.folder_path.toLowerCase().includes(q))
      .sort((a, b) => a.name.localeCompare(b.name))
  }, [screens, searchQuery])

  const allScreensSorted = useMemo(() => [...screens].sort((a, b) => a.name.localeCompare(b.name)), [screens])

  const directSubfolders = useMemo(() => {
    if (selectedFolder === null) return []
    return folders.filter((f) => parentPath(f.path) === selectedFolder).sort((a, b) => a.path.localeCompare(b.path))
  }, [folders, selectedFolder])

  const directScreens = useMemo(() => {
    if (selectedFolder === null) return []
    return screens.filter((s) => s.folder_path === selectedFolder).sort((a, b) => a.name.localeCompare(b.name))
  }, [screens, selectedFolder])

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
          className="p-1.5 rounded text-theme-text-muted hover:text-theme-text-primary hover:bg-theme-border opacity-0 group-hover:opacity-100 transition-opacity"
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
              </div>

              {/* Search */}
              <div className="flex items-center gap-2 mb-4 border border-theme-border rounded-md px-3 py-2 bg-app-panel max-w-md">
                <Search className="w-4 h-4 text-theme-text-muted flex-none" />
                <input
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder="Search screens & folders"
                  className="flex-1 bg-transparent outline-none text-sm text-theme-text-primary placeholder-theme-text-muted"
                />
                {searchQuery && (
                  <button onClick={() => setSearchQuery('')} className="text-theme-text-muted hover:text-theme-text-primary">
                    <X className="w-4 h-4" />
                  </button>
                )}
              </div>

              <div className="flex-1 overflow-hidden grid grid-cols-[240px_1fr] gap-4">
                {/* Sidebar */}
                <div className="overflow-auto border border-theme-border rounded-lg bg-app-panel p-3">
                  <FolderTree
                    folders={folders}
                    selectedFolder={selectedFolder}
                    onSelectAll={selectAll}
                    onSelectFolder={selectFolder}
                    expandedPaths={expandedPaths}
                    onToggleExpand={toggleExpand}
                    matchedFolders={matchedFolders}
                    isSearching={isSearching}
                    onDropScreen={handleMoveScreen}
                    onCreateFolder={handleCreateFolder}
                    onRenameFolder={handleRenameFolder}
                    onDeleteFolder={handleDeleteFolder}
                  />
                </div>

                {/* Main content */}
                <div className="overflow-auto">
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
                  ) : selectedFolder === null ? (
                    <>
                      <FolderBreadcrumb path={null} onNavigate={selectFolder} />
                      {allScreensSorted.length > 0 ? (
                        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
                          {allScreensSorted.map((s) => screenCard(s, true))}
                        </div>
                      ) : (
                        <div className="p-8 rounded-lg border border-dashed border-theme-border text-center">
                          <p className="text-theme-text-muted mb-2">No screens yet.</p>
                          <p className="text-sm text-theme-text-muted">
                            Create your first screen using the buttons above.
                          </p>
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
