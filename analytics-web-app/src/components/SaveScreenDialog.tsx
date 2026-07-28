import { useState, useCallback, useEffect, useRef, useMemo } from 'react'
import { Folder, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { createScreen, ScreenTypeName, ScreenConfig, ScreenApiError } from '@/lib/screens-api'
import { listFolders, FolderInfo } from '@/lib/folders-api'
import { FolderPickerModal } from '@/components/FolderPickerModal'

interface SaveScreenDialogProps {
  isOpen: boolean
  onClose: () => void
  onSaved: (screenName: string) => void
  screenType: ScreenTypeName
  config: ScreenConfig
  /** If provided, pre-fill the name field with a suggested name */
  suggestedName?: string
  /** The current screen's folder, used to default the destination for "Save As". */
  sourceFolderPath?: string
}

/**
 * Normalizes a screen name to URL-safe format.
 * - Converts to lowercase
 * - Replaces spaces and underscores with hyphens
 * - Removes invalid characters
 * - Collapses consecutive hyphens
 */
function normalizeScreenName(name: string): string {
  return name
    .toLowerCase()
    .replace(/[\s_]+/g, '-')
    .replace(/[^a-z0-9-]/g, '')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '')
}

export function SaveScreenDialog({
  isOpen,
  onClose,
  onSaved,
  screenType,
  config,
  suggestedName,
  sourceFolderPath,
}: SaveScreenDialogProps) {
  const [name, setName] = useState(suggestedName || '')
  const [error, setError] = useState<string | null>(null)
  const [isSaving, setIsSaving] = useState(false)
  const [destinationFolder, setDestinationFolder] = useState(sourceFolderPath ?? '')
  const [folders, setFolders] = useState<FolderInfo[]>([])
  const [showFolderPicker, setShowFolderPicker] = useState(false)
  const wasOpenRef = useRef(false)

  // Compute normalized name synchronously to avoid flicker
  const normalizedName = useMemo(() => normalizeScreenName(name), [name])

  // Reset state only when dialog opens (not on every suggestedName change). The
  // dialog is a fixed child of its callers and never unmounts between opens, so
  // this must also reset the destination folder — otherwise reopening "Save As"
  // for a different screen would carry over the previous screen's folder.
  useEffect(() => {
    if (isOpen && !wasOpenRef.current) {
      // Dialog just opened
      setName(suggestedName || '')
      setError(null)
      setIsSaving(false)
      setDestinationFolder(sourceFolderPath ?? '')
      listFolders()
        .then(setFolders)
        .catch(() => setFolders([]))
    }
    wasOpenRef.current = isOpen
  }, [isOpen, suggestedName, sourceFolderPath])

  const handleSave = useCallback(async () => {
    const screenName = normalizedName

    // Client-side validation
    if (screenName.length < 3) {
      setError('Screen name must be at least 3 characters')
      return
    }
    if (screenName.length > 100) {
      setError('Screen name must be at most 100 characters')
      return
    }
    if (!/^[a-z]/.test(screenName)) {
      setError('Screen name must start with a letter')
      return
    }
    if (screenName === 'new') {
      setError('This screen name is reserved')
      return
    }

    setError(null)
    setIsSaving(true)

    try {
      await createScreen({
        name: screenName,
        screen_type: screenType,
        config,
        folder_path: destinationFolder,
      })
      onSaved(screenName)
    } catch (err) {
      if (err instanceof ScreenApiError) {
        if (err.code === 'DUPLICATE_NAME') {
          setError('A screen with this name already exists')
        } else {
          setError(err.message)
        }
      } else {
        setError('Failed to save screen')
      }
    } finally {
      setIsSaving(false)
    }
  }, [normalizedName, screenType, config, destinationFolder, onSaved])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Enter' && !isSaving && normalizedName.length >= 3) {
        handleSave()
      }
      if (e.key === 'Escape') {
        onClose()
      }
    },
    [handleSave, isSaving, normalizedName, onClose]
  )

  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/50" onClick={onClose} />

      {/* Dialog */}
      <div className="relative w-full max-w-md bg-app-panel border border-theme-border rounded-lg shadow-xl">
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
          <h2 className="text-lg font-medium text-theme-text-primary">Save Screen</h2>
          <button
            onClick={onClose}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Body */}
        <div className="p-4">
          <label className="block mb-2">
            <span className="text-sm font-medium text-theme-text-primary">Screen Name</span>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={handleKeyDown}
              placeholder="e.g., My Error Logs"
              autoFocus
              className="mt-1 w-full px-3 py-2 bg-app-bg border border-theme-border rounded-md text-theme-text-primary text-sm placeholder-theme-text-muted focus:outline-none focus:border-accent-link"
            />
          </label>

          {normalizedName && normalizedName !== name.toLowerCase() && (
            <p className="text-xs text-theme-text-secondary mt-1">
              Will be saved as: <span className="font-mono text-accent-link">{normalizedName}</span>
            </p>
          )}

          <div className="flex items-center gap-2 mt-3">
            <span className="text-sm font-medium text-theme-text-primary whitespace-nowrap">Location</span>
            <div className="flex-1 flex items-center gap-1.5 px-2.5 py-1.5 border border-theme-border rounded-md bg-app-bg text-sm text-theme-text-secondary font-mono truncate">
              <Folder className="w-3.5 h-3.5 text-accent-warning flex-none" />
              <span className="truncate">{destinationFolder ? `/${destinationFolder}` : '/'}</span>
            </div>
            <Button variant="outline" size="sm" onClick={() => setShowFolderPicker(true)}>
              Change
            </Button>
          </div>

          {error && (
            <p className="mt-3 text-sm text-accent-error">{error}</p>
          )}
        </div>

        {/* Footer */}
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={onClose} disabled={isSaving}>
            Cancel
          </Button>
          <Button
            onClick={handleSave}
            disabled={isSaving || normalizedName.length < 3}
          >
            {isSaving ? 'Saving...' : 'Save'}
          </Button>
        </div>
      </div>

      <FolderPickerModal
        isOpen={showFolderPicker}
        onClose={() => setShowFolderPicker(false)}
        onSelect={setDestinationFolder}
        currentPath={destinationFolder}
        folders={folders}
        title="Choose location"
      />
    </div>
  )
}
