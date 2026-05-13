import { Suspense, useState, useEffect, useCallback, useRef } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Trash2, Upload, Map as MapIcon } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { ErrorBanner } from '@/components/ErrorBanner'
import { Button } from '@/components/ui/button'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { getConfig } from '@/lib/config'
import {
  MapCatalogEntry,
  deleteMap,
  invalidateMapCatalog,
  fetchMapCatalog,
  formatMapName,
  uploadMap,
} from '@/lib/maps-catalog'

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function formatDate(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString()
}

function MapsPageContent() {
  usePageTitle('Maps')

  const basePath = getConfig().basePath
  const [entries, setEntries] = useState<MapCatalogEntry[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [pendingUpload, setPendingUpload] = useState<File | null>(null)
  const [isUploading, setIsUploading] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)
  const [isDeleting, setIsDeleting] = useState(false)
  const [isDragOver, setIsDragOver] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)

  const loadCatalog = useCallback(async () => {
    setIsLoading(true)
    setError(null)
    invalidateMapCatalog()
    try {
      const data = await fetchMapCatalog(basePath)
      setEntries(data)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load maps')
    } finally {
      setIsLoading(false)
    }
  }, [basePath])

  useEffect(() => {
    loadCatalog()
  }, [loadCatalog])

  const collidesWithExisting = useCallback(
    (name: string) => entries.some((e) => e.file === name),
    [entries]
  )

  const doUpload = useCallback(
    async (file: File) => {
      setIsUploading(true)
      setError(null)
      try {
        await uploadMap(file, basePath)
        await loadCatalog()
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to upload map')
      } finally {
        setIsUploading(false)
      }
    },
    [basePath, loadCatalog]
  )

  const handleFile = useCallback(
    (file: File) => {
      if (collidesWithExisting(file.name)) {
        setPendingUpload(file)
      } else {
        void doUpload(file)
      }
    },
    [collidesWithExisting, doUpload]
  )

  const handleFileInput = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    e.target.value = ''
    if (file) handleFile(file)
  }

  const handleDrop = (e: React.DragEvent) => {
    e.preventDefault()
    setIsDragOver(false)
    const file = e.dataTransfer.files[0]
    if (file) handleFile(file)
  }

  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault()
    setIsDragOver(true)
  }

  const handleDragLeave = () => {
    setIsDragOver(false)
  }

  const handleConfirmReplace = async () => {
    if (!pendingUpload) return
    // Keep the dialog open across the upload so the spinner inside
    // `ConfirmDialog` (driven by `isLoading={isUploading}`) is visible.
    // Mirrors the delete-confirm flow.
    const file = pendingUpload
    await doUpload(file)
    setPendingUpload(null)
  }

  const handleDelete = async () => {
    if (!deleteTarget) return
    setIsDeleting(true)
    setError(null)
    try {
      await deleteMap(deleteTarget, basePath)
      setDeleteTarget(null)
      await loadCatalog()
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to delete map')
      setDeleteTarget(null)
    } finally {
      setIsDeleting(false)
    }
  }

  return (
    <AuthGuard requireAdmin>
      <PageLayout onRefresh={loadCatalog}>
        <div className="p-6 flex flex-col h-full">
          <div className="flex items-center gap-1.5 text-sm text-theme-text-muted mb-4">
            <AppLink href="/admin" className="text-accent-link hover:underline">
              Admin
            </AppLink>
            <span>/</span>
            <span>Maps</span>
          </div>

          <div className="flex items-center justify-between mb-6">
            <div>
              <h1 className="text-2xl font-semibold text-theme-text-primary">Maps</h1>
              <p className="mt-1 text-theme-text-secondary">
                Upload and manage GLB map assets served to map cells.
              </p>
            </div>
            <Button
              onClick={() => fileInputRef.current?.click()}
              disabled={isUploading}
              className="gap-1.5"
            >
              <Upload className="w-4 h-4" />
              Upload Map
            </Button>
            <input
              ref={fileInputRef}
              type="file"
              accept=".glb,model/gltf-binary"
              className="hidden"
              onChange={handleFileInput}
              aria-label="Choose GLB file"
            />
          </div>

          {error && (
            <ErrorBanner title="Maps error" message={error} onDismiss={() => setError(null)} />
          )}

          <div
            className={`border-2 border-dashed rounded-xl p-8 text-center transition-colors cursor-pointer mb-6 ${
              isDragOver
                ? 'border-accent-link bg-accent-link/5'
                : 'border-theme-border hover:border-accent-link hover:bg-accent-link/5'
            } ${isUploading ? 'opacity-50 pointer-events-none' : ''}`}
            role="button"
            aria-label="Drop a .glb file here or click to browse"
            onDrop={handleDrop}
            onDragOver={handleDragOver}
            onDragLeave={handleDragLeave}
            onClick={() => fileInputRef.current?.click()}
          >
            <Upload className="w-8 h-8 mx-auto mb-2 text-theme-text-muted opacity-50" />
            <p className="text-sm font-medium text-theme-text-primary mb-1">
              {isUploading ? 'Uploading...' : 'Drop a .glb file here'}
            </p>
            <p className="text-xs text-theme-text-muted">
              or <span className="text-accent-link underline">browse your files</span>
            </p>
          </div>

          <ConfirmDialog
            isOpen={pendingUpload !== null}
            onClose={() => setPendingUpload(null)}
            onConfirm={handleConfirmReplace}
            title="Replace existing map?"
            message={
              pendingUpload
                ? `A map named "${pendingUpload.name}" already exists. Replace it?`
                : ''
            }
            confirmLabel="Replace"
            isLoading={isUploading}
            variant="danger"
          />

          <ConfirmDialog
            isOpen={deleteTarget !== null}
            onClose={() => setDeleteTarget(null)}
            onConfirm={handleDelete}
            title="Delete Map"
            message={`Are you sure you want to delete "${deleteTarget}"? This action cannot be undone.`}
            confirmLabel="Delete"
            isLoading={isDeleting}
            variant="danger"
          />

          {isLoading ? (
            <div className="flex-1 flex items-center justify-center">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading maps...</span>
              </div>
            </div>
          ) : entries.length === 0 ? (
            <div className="flex-1 flex flex-col items-center justify-center text-center">
              <MapIcon className="w-10 h-10 text-theme-text-muted opacity-40 mb-3" />
              <p className="text-theme-text-muted mb-4">No maps uploaded yet.</p>
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
                      Size
                    </th>
                    <th className="text-left p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Last Modified
                    </th>
                    <th className="text-right p-2.5 px-4 text-xs font-semibold text-theme-text-muted uppercase tracking-wider">
                      Actions
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {entries.map((entry) => (
                    <tr
                      key={entry.file}
                      className="border-t border-theme-border hover:bg-accent-link/5"
                    >
                      <td className="p-2.5 px-4">
                        <span className="text-theme-text-primary font-medium">
                          {formatMapName(entry.file)}
                        </span>
                        <span className="text-theme-text-muted ml-2 text-xs">{entry.file}</span>
                      </td>
                      <td className="p-2.5 px-4 text-theme-text-secondary text-sm">
                        {formatSize(entry.size)}
                      </td>
                      <td className="p-2.5 px-4 text-theme-text-secondary text-sm">
                        {formatDate(entry.last_modified)}
                      </td>
                      <td className="p-2.5 px-4 text-right">
                        <button
                          onClick={() => setDeleteTarget(entry.file)}
                          className="p-1.5 rounded text-theme-text-muted hover:text-red-400 hover:bg-red-400/10 transition-colors"
                          title="Delete"
                          aria-label={`Delete ${entry.file}`}
                        >
                          <Trash2 className="w-4 h-4" />
                        </button>
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

export default function MapsPage() {
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
      <MapsPageContent />
    </Suspense>
  )
}
