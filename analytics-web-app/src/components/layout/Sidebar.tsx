import { useCallback, useEffect, useRef, useState } from 'react'
import { useLocation, useNavigate, useSearchParams } from 'react-router'
import { LayoutGrid, Search, Wrench, X } from 'lucide-react'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { useAuth } from '@/lib/auth'
import { FolderTree, ancestorPaths, parentOf } from '@/components/FolderTree'
import { listFolders, createFolder, moveFolder, deleteFolder, screenMatchesQuery, FolderInfo } from '@/lib/folders-api'
import { listScreens, Screen, ScreenApiError } from '@/lib/screens-api'
import { notifyFoldersChanged, useFoldersChangedListener } from '@/lib/folders-sync'
import { useMoveScreen } from '@/hooks/useMoveScreen'

interface NavItem {
  href: string
  icon: React.ReactNode
  label: string
  matchPaths?: string[]
}

const navItems: NavItem[] = [
  {
    href: '/processes',
    icon: <LayoutGrid className="w-5 h-5" />,
    label: 'Processes',
    matchPaths: ['/processes', '/process', '/process_log', '/process_metrics', '/performance_analysis'],
  },
]

function computeMatchedFolders(screens: Screen[], query: string): Set<string> {
  const set = new Set<string>()
  const q = query.trim().toLowerCase()
  if (!q) return set
  for (const s of screens) {
    if (screenMatchesQuery(s, q)) {
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

function computeMatchedScreens(screens: Screen[], query: string): Set<string> {
  const set = new Set<string>()
  const q = query.trim().toLowerCase()
  if (!q) return set
  for (const s of screens) {
    if (s.name.toLowerCase().includes(q)) set.add(s.name)
  }
  return set
}

export function Sidebar() {
  const location = useLocation()
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const { user } = useAuth()
  const asideRef = useRef<HTMLElement>(null)

  const [isOpen, setIsOpen] = useState(false)
  const [folders, setFolders] = useState<FolderInfo[]>([])
  const [screens, setScreens] = useState<Screen[]>([])
  const [foldersError, setFoldersError] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [expandedPaths, setExpandedPaths] = useState<Set<string>>(new Set())
  const [searchQuery, setSearchQuery] = useState('')

  const pathname = location.pathname
  const isScreensPage = pathname === '/screens'
  const selectedFolder = isScreensPage ? searchParams.get('folder') ?? '' : null

  const screenPageMatch = pathname.match(/^\/screen\/([^/]+)$/)
  const activeScreenName = screenPageMatch ? screenPageMatch[1] : null
  const activeScreen = activeScreenName ? screens.find((s) => s.name === activeScreenName) : undefined

  const isActive = (item: NavItem) => {
    if (item.matchPaths) {
      return item.matchPaths.some((path) => pathname.startsWith(path))
    }
    return pathname === item.href
  }

  const adminItem: NavItem = {
    href: '/admin',
    icon: <Wrench className="w-5 h-5" />,
    label: 'Admin',
    matchPaths: ['/admin'],
  }

  const loadFolders = useCallback(async () => {
    setFoldersError(null)
    try {
      const [foldersData, screensData] = await Promise.all([listFolders(), listScreens()])
      setFolders(foldersData)
      setScreens(screensData)
    } catch (err) {
      setFoldersError(err instanceof ScreenApiError ? err.message : 'Failed to load folders')
    }
  }, [])

  useEffect(() => {
    loadFolders()
  }, [loadFolders])

  useFoldersChangedListener(loadFolders)

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

  // Also reveal the folder containing whatever screen is currently open.
  useEffect(() => {
    const folderPath = activeScreen?.folder_path
    if (folderPath) {
      setExpandedPaths((prev) => {
        const next = new Set(prev)
        ancestorPaths(folderPath).forEach((p) => next.add(p))
        next.add(folderPath)
        return next
      })
    }
  }, [activeScreen])

  // Keep the search box in sync with the URL (direct links, back/forward).
  useEffect(() => {
    setSearchQuery(isScreensPage ? searchParams.get('q') ?? '' : '')
  }, [isScreensPage, searchParams])

  const toggleExpand = useCallback((path: string) => {
    setExpandedPaths((prev) => {
      const next = new Set(prev)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  }, [])

  const buildFolderUrl = useCallback(
    (path: string) => {
      const next = new URLSearchParams(isScreensPage ? searchParams : undefined)
      next.set('folder', path)
      next.delete('q')
      return `/screens?${next.toString()}`
    },
    [isScreensPage, searchParams]
  )

  const goToFolder = useCallback(
    (path: string) => {
      const url = buildFolderUrl(path)
      if (isScreensPage) navigate(url, { replace: true })
      else navigate(url)
      setSearchQuery('')
    },
    [isScreensPage, navigate, buildFolderUrl]
  )

  const screenHref = useCallback((name: string) => `/screen/${name}`, [])

  const updateSearch = useCallback(
    (value: string) => {
      setSearchQuery(value)
      if (!isScreensPage && !value) return
      const next = new URLSearchParams(isScreensPage ? searchParams : undefined)
      if (value) next.set('q', value)
      else next.delete('q')
      const url = `/screens?${next.toString()}`
      if (isScreensPage) navigate(url, { replace: true })
      else navigate(url)
    },
    [isScreensPage, searchParams, navigate]
  )

  const handleMoveScreen = useMoveScreen(screens, loadFolders, setActionError)

  const handleCreateFolder = useCallback(
    async (parent: string, name: string) => {
      const path = parent ? `${parent}/${name}` : name
      setActionError(null)
      try {
        await createFolder(path)
        notifyFoldersChanged()
        await loadFolders()
      } catch (err) {
        setActionError(
          err instanceof ScreenApiError ? `Failed to create folder: ${err.message}` : 'Failed to create folder'
        )
      }
    },
    [loadFolders]
  )

  const handleRenameFolder = useCallback(
    async (path: string, newPath: string) => {
      setActionError(null)
      try {
        await moveFolder(path, newPath)
        setExpandedPaths((prev) => {
          const next = new Set<string>()
          for (const p of prev) {
            if (p === path) next.add(newPath)
            else if (p.startsWith(`${path}/`)) next.add(`${newPath}${p.slice(path.length)}`)
            else next.add(p)
          }
          return next
        })
        if (selectedFolder === path) {
          goToFolder(newPath)
        } else if (selectedFolder && selectedFolder.startsWith(`${path}/`)) {
          goToFolder(`${newPath}${selectedFolder.slice(path.length)}`)
        }
        notifyFoldersChanged()
        await loadFolders()
      } catch (err) {
        setActionError(
          err instanceof ScreenApiError ? `Failed to rename folder: ${err.message}` : 'Failed to rename folder'
        )
      }
    },
    [loadFolders, selectedFolder, goToFolder]
  )

  const handleDeleteFolder = useCallback(
    async (path: string) => {
      setActionError(null)
      try {
        await deleteFolder(path)
        if (selectedFolder === path) {
          goToFolder(parentOf(path))
        }
        notifyFoldersChanged()
        await loadFolders()
      } catch (err) {
        setActionError(
          err instanceof ScreenApiError ? `Failed to delete folder: ${err.message}` : 'Failed to delete folder'
        )
      }
    },
    [loadFolders, selectedFolder, goToFolder]
  )

  const matchedFolders = computeMatchedFolders(screens, searchQuery)
  const matchedScreens = computeMatchedScreens(screens, searchQuery)
  const isSearching = searchQuery.trim().length > 0

  const closeUnlessFocusInside = () => {
    if (asideRef.current && document.activeElement && asideRef.current.contains(document.activeElement)) {
      return
    }
    setIsOpen(false)
  }

  return (
    <aside
      ref={asideRef}
      className="relative hidden sm:flex flex-none"
      onMouseEnter={() => setIsOpen(true)}
      onMouseLeave={closeUnlessFocusInside}
      onBlurCapture={(e) => {
        if (!asideRef.current?.contains(e.relatedTarget as Node)) setIsOpen(false)
      }}
    >
      <div className="flex w-14 bg-app-sidebar border-r border-theme-border flex-col py-3">
        <nav className="flex flex-col gap-1">
          {navItems.map((item) => (
            <AppLink
              key={item.href}
              href={item.href}
              className={`flex items-center justify-center w-10 h-10 mx-2 rounded-md transition-colors ${
                isActive(item)
                  ? 'bg-app-card text-accent-link'
                  : 'text-theme-text-secondary hover:bg-theme-border hover:text-theme-text-primary'
              }`}
              title={item.label}
            >
              {item.icon}
            </AppLink>
          ))}
        </nav>
        {user?.is_admin && (
          <div className="mt-auto">
            <div className="h-px bg-theme-border mx-2 mb-1" />
            <nav className="flex flex-col gap-1">
              <AppLink
                href={adminItem.href}
                className={`flex items-center justify-center w-10 h-10 mx-2 rounded-md transition-colors ${
                  isActive(adminItem)
                    ? 'bg-app-card text-accent-link'
                    : 'text-theme-text-secondary hover:bg-theme-border hover:text-theme-text-primary'
                }`}
                title={adminItem.label}
              >
                {adminItem.icon}
              </AppLink>
            </nav>
          </div>
        )}
      </div>

      {isOpen && (
        <div className="absolute top-0 left-14 h-full w-64 bg-app-sidebar border-r border-theme-border shadow-xl z-40 flex flex-col p-3 gap-2">
          <div className="flex items-center gap-2 border border-theme-border rounded-md px-2 py-1.5 bg-app-panel flex-none">
            <Search className="w-3.5 h-3.5 text-theme-text-muted flex-none" />
            <input
              value={searchQuery}
              onChange={(e) => updateSearch(e.target.value)}
              placeholder="Search screens & folders"
              className="flex-1 min-w-0 bg-transparent outline-hidden text-sm text-theme-text-primary placeholder-theme-text-muted"
            />
            {searchQuery && (
              <button
                onClick={() => updateSearch('')}
                className="text-theme-text-muted hover:text-theme-text-primary flex-none"
              >
                <X className="w-3.5 h-3.5" />
              </button>
            )}
          </div>

          {foldersError && (
            <ErrorBanner title="Error" message={foldersError} onRetry={loadFolders} />
          )}
          {actionError && (
            <ErrorBanner title="Error" message={actionError} onDismiss={() => setActionError(null)} />
          )}

          <div className="flex-1 overflow-auto">
            <FolderTree
              folders={folders}
              screens={screens}
              selectedFolder={selectedFolder}
              onSelectFolder={(_, e) => {
                if (e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return
                setSearchQuery('')
              }}
              folderHref={buildFolderUrl}
              folderNavReplace={isScreensPage}
              selectedScreen={activeScreenName}
              onSelectScreen={(_, e) => {
                if (e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return
                setSearchQuery('')
              }}
              screenHref={screenHref}
              expandedPaths={expandedPaths}
              onToggleExpand={toggleExpand}
              matchedFolders={matchedFolders}
              matchedScreens={matchedScreens}
              isSearching={isSearching}
              onDropScreen={handleMoveScreen}
              onCreateFolder={handleCreateFolder}
              onRenameFolder={handleRenameFolder}
              onDeleteFolder={handleDeleteFolder}
            />
          </div>
        </div>
      )}
    </aside>
  )
}
