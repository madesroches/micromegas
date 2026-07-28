import { useState } from 'react'
import { ChevronRight, Home } from 'lucide-react'

interface FolderBreadcrumbProps {
  /** Current folder path, or null when showing search results. */
  path: string | null
  onNavigate: (path: string) => void
  onDropScreen?: (screenName: string, destPath: string) => void
}

export function FolderBreadcrumb({ path, onNavigate, onDropScreen }: FolderBreadcrumbProps) {
  const [dropTarget, setDropTarget] = useState<string | null>(null)

  if (path === null) {
    return (
      <div className="flex items-center gap-1 text-sm text-theme-text-primary font-medium mb-3">
        Search results
      </div>
    )
  }

  const segments = path === '' ? [] : path.split('/')

  const dropHandlers = (segmentPath: string) =>
    onDropScreen
      ? {
          onDragOver: (e: React.DragEvent) => {
            e.preventDefault()
            e.dataTransfer.dropEffect = 'move'
            setDropTarget(segmentPath)
          },
          onDragLeave: () => setDropTarget((p) => (p === segmentPath ? null : p)),
          onDrop: (e: React.DragEvent) => {
            e.preventDefault()
            setDropTarget(null)
            const name = e.dataTransfer.getData('text/plain')
            if (name) onDropScreen(name, segmentPath)
          },
        }
      : {}

  const crumbClass = (segmentPath: string, isCurrent: boolean) =>
    `px-1.5 py-0.5 rounded ${isCurrent ? 'text-theme-text-primary font-semibold' : 'cursor-pointer text-theme-text-secondary hover:bg-app-card hover:text-theme-text-primary'} ${
      dropTarget === segmentPath ? 'ring-1 ring-inset ring-accent-link bg-accent-link/20' : ''
    }`

  return (
    <div className="flex items-center gap-1 text-sm mb-3 flex-wrap">
      <span
        role="button"
        tabIndex={0}
        onClick={() => onNavigate('')}
        onKeyDown={(e) => e.key === 'Enter' && onNavigate('')}
        {...dropHandlers('')}
        className={`flex items-center ${crumbClass('', segments.length === 0)}`}
      >
        <Home className="w-3.5 h-3.5" />
      </span>
      {segments.map((segment, i) => {
        const segmentPath = segments.slice(0, i + 1).join('/')
        const isCurrent = i === segments.length - 1
        return (
          <span key={segmentPath} className="flex items-center gap-1">
            <ChevronRight className="w-3.5 h-3.5 text-theme-text-muted" />
            <span
              role="button"
              tabIndex={0}
              onClick={() => !isCurrent && onNavigate(segmentPath)}
              onKeyDown={(e) => e.key === 'Enter' && !isCurrent && onNavigate(segmentPath)}
              {...dropHandlers(segmentPath)}
              className={crumbClass(segmentPath, isCurrent)}
            >
              {segment}
            </span>
          </span>
        )
      })}
    </div>
  )
}
