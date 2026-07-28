import { useState } from 'react'
import { Check, Folder, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { FolderInfo } from '@/lib/folders-api'

interface FolderPickerModalProps {
  isOpen: boolean
  onClose: () => void
  onSelect: (path: string) => void
  /** The item's current folder, highlighted with a checkmark. */
  currentPath?: string
  folders: FolderInfo[]
  title?: string
}

function label(path: string): string {
  return path === '' ? 'Home' : (path.split('/').pop() ?? path)
}

function depthOf(path: string): number {
  return path === '' ? 0 : path.split('/').length
}

/**
 * Shared destination-folder picker — backs both the screen-card kebab's
 * "Move to folder" action and SaveScreenDialog's "Change" location button.
 */
export function FolderPickerModal({
  isOpen,
  onClose,
  onSelect,
  currentPath,
  folders,
  title = 'Move to folder',
}: FolderPickerModalProps) {
  const [newPath, setNewPath] = useState('')

  if (!isOpen) return null

  const allPaths = ['', ...folders.map((f) => f.path).sort((a, b) => a.localeCompare(b))]

  const handlePick = (path: string) => {
    onSelect(path)
    onClose()
  }

  const handleMoveToNew = () => {
    const cleaned = newPath.trim().replace(/^\/+|\/+$/g, '')
    if (!cleaned) return
    onSelect(cleaned)
    setNewPath('')
    onClose()
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={onClose} />

      <div className="relative w-full max-w-md bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
          <h2 className="text-lg font-medium text-theme-text-primary">{title}</h2>
          <button
            onClick={onClose}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        <div className="p-4">
          <span className="text-sm font-medium text-theme-text-primary">
            Choose destination folder
          </span>
          <div className="mt-2 max-h-56 overflow-auto border border-theme-border rounded-md">
            {allPaths.map((path) => (
              <div
                key={path || '__root__'}
                role="button"
                tabIndex={0}
                onClick={() => handlePick(path)}
                onKeyDown={(e) => e.key === 'Enter' && handlePick(path)}
                style={{ paddingLeft: `${depthOf(path) * 14 + 10}px` }}
                className="flex items-center gap-1.5 py-1.5 pr-2 text-sm cursor-pointer hover:bg-app-card"
              >
                {path === currentPath ? (
                  <Check className="w-3.5 h-3.5 flex-none text-accent-link" />
                ) : (
                  <span className="w-3.5 h-3.5 flex-none" />
                )}
                <Folder className="w-4 h-4 flex-none text-accent-warning" />
                <span className="truncate">{label(path)}</span>
              </div>
            ))}
          </div>

          <input
            value={newPath}
            onChange={(e) => setNewPath(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleMoveToNew()}
            placeholder="Or type a new folder path, e.g. team/reports"
            className="mt-3 w-full px-3 py-2 bg-app-bg border border-theme-border rounded-md text-theme-text-primary text-sm placeholder-theme-text-muted focus:outline-none focus:border-accent-link"
          />
        </div>

        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={handleMoveToNew} disabled={!newPath.trim()}>
            Move to new folder
          </Button>
        </div>
      </div>
    </div>
  )
}
