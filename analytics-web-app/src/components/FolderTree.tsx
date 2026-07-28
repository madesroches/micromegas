import { useMemo, useState } from 'react'
import { ChevronRight, FileText, Folder, FolderOpen, Home, MoreVertical, Plus } from 'lucide-react'
import { FolderInfo } from '@/lib/folders-api'
import { Screen } from '@/lib/screens-api'
import { ConfirmDialog } from '@/components/ConfirmDialog'

export interface FolderTreeNode {
  name: string
  path: string
  children: FolderTreeNode[]
  screens: Screen[]
}

/** Builds a nested tree from the flat `GET /folders` response (root excluded), attaching screens to their folder. */
export function buildFolderTree(folders: FolderInfo[], screens: Screen[] = []): FolderTreeNode {
  const root: FolderTreeNode = { name: '', path: '', children: [], screens: [] }
  const nodeByPath = new Map<string, FolderTreeNode>([['', root]])

  const sorted = [...folders].sort((a, b) => a.path.localeCompare(b.path))
  for (const folder of sorted) {
    const segments = folder.path.split('/')
    let parentPath = ''
    let currentPath = ''
    for (const segment of segments) {
      currentPath = currentPath ? `${currentPath}/${segment}` : segment
      if (!nodeByPath.has(currentPath)) {
        const node: FolderTreeNode = { name: segment, path: currentPath, children: [], screens: [] }
        nodeByPath.set(currentPath, node)
        nodeByPath.get(parentPath)?.children.push(node)
      }
      parentPath = currentPath
    }
  }

  for (const screen of screens) {
    nodeByPath.get(screen.folder_path)?.screens.push(screen)
  }

  const sortChildren = (node: FolderTreeNode) => {
    node.children.sort((a, b) => a.name.localeCompare(b.name))
    node.screens.sort((a, b) => a.name.localeCompare(b.name))
    node.children.forEach(sortChildren)
  }
  sortChildren(root)
  return root
}

/**
 * Character-transform-only normalization for a folder name segment (lowercase,
 * hyphenate, strip invalid characters) — mirrors `normalizeScreenName`'s
 * transform, but without its paired screen-only length/reserved-word checks.
 * Folder segments use the backend folder-segment rules instead (min length 1,
 * no reserved-word check).
 */
export function normalizeFolderSegment(name: string): string {
  return name
    .toLowerCase()
    .replace(/[\s_]+/g, '-')
    .replace(/[^a-z0-9-]/g, '')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '')
}

/** All ancestor paths (exclusive of `path` itself) needed to reveal it in the tree. */
export function ancestorPaths(path: string): string[] {
  if (!path) return []
  const segments = path.split('/')
  const ancestors: string[] = []
  let current = ''
  for (let i = 0; i < segments.length - 1; i++) {
    current = current ? `${current}/${segments[i]}` : segments[i]
    ancestors.push(current)
  }
  return ancestors
}

interface FolderTreeProps {
  folders: FolderInfo[]
  screens: Screen[]
  /**
   * Currently selected folder path ('' is the Home/root folder), or null
   * when no folder is active (e.g. the sidebar is shown on a non-Screens page).
   */
  selectedFolder: string | null
  onSelectFolder: (path: string) => void
  /** Name of the screen currently open (e.g. on its screen page), or null/undefined if none. */
  selectedScreen?: string | null
  onSelectScreen: (screenName: string) => void
  expandedPaths: Set<string>
  onToggleExpand: (path: string) => void
  /** Folders (or their ancestors) that contain a search match; drives auto-expand + a dot marker. */
  matchedFolders?: Set<string>
  /** Screen names that match the current search query; drives a dot marker. */
  matchedScreens?: Set<string>
  isSearching?: boolean
  onDropScreen: (screenName: string, destPath: string) => void
  onCreateFolder: (parentPath: string, name: string) => void
  onRenameFolder: (path: string, newPath: string) => void
  onDeleteFolder: (path: string) => void
}

export function parentOf(path: string): string {
  const idx = path.lastIndexOf('/')
  return idx === -1 ? '' : path.slice(0, idx)
}

export function FolderTree({
  folders,
  screens,
  selectedFolder,
  onSelectFolder,
  selectedScreen,
  onSelectScreen,
  expandedPaths,
  onToggleExpand,
  matchedFolders,
  matchedScreens,
  isSearching,
  onDropScreen,
  onCreateFolder,
  onRenameFolder,
  onDeleteFolder,
}: FolderTreeProps) {
  const tree = useMemo(() => buildFolderTree(folders, screens), [folders, screens])
  const [dropTargetPath, setDropTargetPath] = useState<string | null>(null)
  const [creatingUnder, setCreatingUnder] = useState<string | null>(null)
  const [newFolderName, setNewFolderName] = useState('')
  const [menuFor, setMenuFor] = useState<string | null>(null)
  const [renamingPath, setRenamingPath] = useState<string | null>(null)
  const [renameValue, setRenameValue] = useState('')
  const [deletingPath, setDeletingPath] = useState<string | null>(null)

  const startCreating = (parentPath: string) => {
    setCreatingUnder(parentPath)
    setNewFolderName('')
  }

  const normalizedNewFolderName = useMemo(() => normalizeFolderSegment(newFolderName), [newFolderName])

  const commitCreate = () => {
    if (normalizedNewFolderName && creatingUnder !== null) {
      onCreateFolder(creatingUnder, normalizedNewFolderName)
    }
    setCreatingUnder(null)
    setNewFolderName('')
  }

  const startRenaming = (node: FolderTreeNode) => {
    setMenuFor(null)
    setRenamingPath(node.path)
    setRenameValue(node.name)
  }

  const normalizedRenameValue = useMemo(() => normalizeFolderSegment(renameValue), [renameValue])

  const commitRename = (node: FolderTreeNode) => {
    if (normalizedRenameValue && normalizedRenameValue !== node.name) {
      const newPath = parentOf(node.path) ? `${parentOf(node.path)}/${normalizedRenameValue}` : normalizedRenameValue
      onRenameFolder(node.path, newPath)
    }
    setRenamingPath(null)
    setRenameValue('')
  }

  const renameInput = (node: FolderTreeNode) => (
    <>
      <input
        autoFocus
        value={renameValue}
        onChange={(e) => setRenameValue(e.target.value)}
        onClick={(e) => e.stopPropagation()}
        onBlur={() => commitRename(node)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commitRename(node)
          if (e.key === 'Escape') setRenamingPath(null)
        }}
        className="flex-1 min-w-0 px-1.5 py-0.5 text-sm bg-app-bg border border-accent-link rounded text-theme-text-primary outline-none"
      />
    </>
  )

  const newFolderInput = () => (
    <>
      <input
        autoFocus
        value={newFolderName}
        onChange={(e) => setNewFolderName(e.target.value)}
        onBlur={commitCreate}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commitCreate()
          if (e.key === 'Escape') setCreatingUnder(null)
        }}
        placeholder="Folder name"
        className="w-full px-2 py-1 text-xs bg-app-bg border border-accent-link rounded text-theme-text-primary outline-none"
      />
      {normalizedNewFolderName && normalizedNewFolderName !== newFolderName.toLowerCase() && (
        <p className="text-[11px] text-theme-text-muted mt-0.5">
          Will be saved as: <span className="font-mono text-accent-link">{normalizedNewFolderName}</span>
        </p>
      )}
    </>
  )

  const dropHandlers = (path: string) => ({
    onDragOver: (e: React.DragEvent) => {
      e.preventDefault()
      e.dataTransfer.dropEffect = 'move'
      setDropTargetPath(path)
    },
    onDragLeave: () => setDropTargetPath((p) => (p === path ? null : p)),
    onDrop: (e: React.DragEvent) => {
      e.preventDefault()
      setDropTargetPath(null)
      const name = e.dataTransfer.getData('text/plain')
      if (name) onDropScreen(name, path)
    },
  })

  const renderScreen = (screen: Screen, depth: number) => (
    <div
      key={`screen:${screen.name}`}
      role="button"
      tabIndex={0}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData('text/plain', screen.name)
        e.dataTransfer.effectAllowed = 'move'
      }}
      onClick={() => onSelectScreen(screen.name)}
      onKeyDown={(e) => e.key === 'Enter' && onSelectScreen(screen.name)}
      style={{ paddingLeft: `${depth * 14 + 8}px` }}
      className={`group flex items-center gap-1.5 py-1.5 pr-2 rounded-md cursor-pointer text-sm transition-colors ${
        selectedScreen === screen.name
          ? 'bg-accent-link/15 text-theme-text-primary'
          : 'text-theme-text-secondary hover:bg-app-card hover:text-theme-text-primary'
      }`}
    >
      <span className="w-3.5 h-3.5 flex-none" />
      <FileText className="w-4 h-4 flex-none text-accent-link" />
      <span className="flex-1 truncate">{screen.name}</span>
      {isSearching && matchedScreens?.has(screen.name) && (
        <span className="w-1.5 h-1.5 rounded-full bg-accent-warning flex-none" />
      )}
    </div>
  )

  const renderNode = (node: FolderTreeNode, depth: number) => {
    const isOpen = expandedPaths.has(node.path) || (isSearching && matchedFolders?.has(node.path))
    const hasChildren = node.children.length > 0 || node.screens.length > 0
    const hasMatch = isSearching && matchedFolders?.has(node.path)

    return (
      <div key={node.path}>
        <div
          role="button"
          tabIndex={0}
          onClick={() => onSelectFolder(node.path)}
          onKeyDown={(e) => e.key === 'Enter' && onSelectFolder(node.path)}
          {...dropHandlers(node.path)}
          style={{ paddingLeft: `${depth * 14 + 8}px` }}
          className={`group flex items-center gap-1.5 py-1.5 pr-2 rounded-md cursor-pointer text-sm transition-colors ${
            selectedFolder === node.path
              ? 'bg-accent-link/15 text-theme-text-primary'
              : 'text-theme-text-secondary hover:bg-app-card hover:text-theme-text-primary'
          } ${dropTargetPath === node.path ? 'ring-1 ring-inset ring-accent-link bg-accent-link/20' : ''}`}
        >
          {hasChildren ? (
            <ChevronRight
              className={`w-3.5 h-3.5 flex-none text-theme-text-muted transition-transform ${isOpen ? 'rotate-90' : ''}`}
              onClick={(e) => {
                e.stopPropagation()
                onToggleExpand(node.path)
              }}
            />
          ) : (
            <span className="w-3.5 h-3.5 flex-none" />
          )}
          {isOpen ? (
            <FolderOpen className="w-4 h-4 flex-none text-accent-warning" />
          ) : (
            <Folder className="w-4 h-4 flex-none text-accent-warning" />
          )}
          {renamingPath === node.path ? (
            renameInput(node)
          ) : (
            <span className="flex-1 truncate">{node.name}</span>
          )}
          {hasMatch && <span className="w-1.5 h-1.5 rounded-full bg-accent-warning flex-none" />}
          <button
            onClick={(e) => {
              e.stopPropagation()
              if (!expandedPaths.has(node.path)) onToggleExpand(node.path)
              startCreating(node.path)
            }}
            title="New subfolder"
            className="opacity-0 group-hover:opacity-100 p-0.5 rounded text-theme-text-muted hover:text-theme-text-primary hover:bg-theme-border transition-opacity"
          >
            <Plus className="w-3.5 h-3.5" />
          </button>
          <div className="relative">
            <button
              onClick={(e) => {
                e.stopPropagation()
                setMenuFor(menuFor === node.path ? null : node.path)
              }}
              title="Folder actions"
              className="opacity-0 group-hover:opacity-100 p-0.5 rounded text-theme-text-muted hover:text-theme-text-primary hover:bg-theme-border transition-opacity"
            >
              <MoreVertical className="w-3.5 h-3.5" />
            </button>
            {menuFor === node.path && (
              <div
                role="menu"
                onClick={(e) => e.stopPropagation()}
                className="absolute right-0 top-full mt-1 w-32 bg-app-card border border-theme-border-hover rounded-md shadow-lg z-10 overflow-hidden"
              >
                <button
                  onClick={() => startRenaming(node)}
                  className="w-full text-left px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border"
                >
                  Rename
                </button>
                <button
                  onClick={() => {
                    setMenuFor(null)
                    setDeletingPath(node.path)
                  }}
                  className="w-full text-left px-3 py-2 text-sm text-red-400 hover:bg-theme-border"
                >
                  Delete
                </button>
              </div>
            )}
          </div>
        </div>
        {creatingUnder === node.path && (
          <div style={{ paddingLeft: `${(depth + 1) * 14 + 8}px` }} className="py-1 pr-2">
            {newFolderInput()}
          </div>
        )}
        {isOpen && node.children.map((child) => renderNode(child, depth + 1))}
        {isOpen && node.screens.map((screen) => renderScreen(screen, depth + 1))}
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-1 h-full" onClick={() => setMenuFor(null)}>
      <div
        role="button"
        tabIndex={0}
        onClick={() => onSelectFolder('')}
        onKeyDown={(e) => e.key === 'Enter' && onSelectFolder('')}
        {...dropHandlers('')}
        className={`flex items-center gap-1.5 py-1.5 px-2 rounded-md cursor-pointer text-sm transition-colors ${
          selectedFolder === '' && !isSearching
            ? 'bg-accent-link/15 text-theme-text-primary'
            : 'text-theme-text-secondary hover:bg-app-card hover:text-theme-text-primary'
        } ${dropTargetPath === '' ? 'ring-1 ring-inset ring-accent-link bg-accent-link/20' : ''}`}
      >
        <Home className="w-4 h-4 flex-none text-theme-text-muted" />
        <span>Home</span>
      </div>

      <div className="h-px bg-theme-border my-1" />

      <div className="flex-1 overflow-auto">
        {creatingUnder === '' && <div className="py-1 pr-2 pl-2">{newFolderInput()}</div>}
        {tree.children.map((child) => renderNode(child, 0))}
        {tree.screens.map((screen) => renderScreen(screen, 0))}
      </div>

      <button
        onClick={() => startCreating(selectedFolder ?? '')}
        className="flex items-center justify-center gap-1.5 text-xs text-theme-text-secondary border border-dashed border-theme-border-hover rounded-md py-1.5 hover:text-theme-text-primary hover:border-accent-link hover:bg-accent-link/5 transition-colors"
      >
        <Plus className="w-3.5 h-3.5" />
        New folder{selectedFolder ? ` in ${selectedFolder}` : ''}
      </button>

      <ConfirmDialog
        isOpen={deletingPath !== null}
        onClose={() => setDeletingPath(null)}
        onConfirm={() => {
          if (deletingPath) onDeleteFolder(deletingPath)
          setDeletingPath(null)
        }}
        title="Delete Folder"
        message={`Are you sure you want to delete "${deletingPath}"? The folder must be empty (no screens, no subfolders).`}
        confirmLabel="Delete"
        variant="danger"
      />
    </div>
  )
}
