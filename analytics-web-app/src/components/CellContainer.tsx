import { ReactNode, useState, useRef, useEffect, forwardRef, HTMLAttributes, useCallback } from 'react'
import { ChevronDown, ChevronRight, Play, RotateCcw, MoreVertical, Trash2, GripVertical } from 'lucide-react'
import { CellType, CellStatus, getCellTypeMetadata } from '@/lib/screen-renderers/cell-registry'
import { Button } from '@/components/ui/button'
import { ResizeHandle } from '@/components/ResizeHandle'

interface CellContainerProps extends Omit<HTMLAttributes<HTMLDivElement>, 'children'> {
  /** Cell name/title */
  name: string
  /** Cell type */
  type: CellType
  /** Current execution status */
  status: CellStatus
  /** Error message if status is 'error' */
  error?: string
  /** Whether the cell is collapsed */
  collapsed?: boolean
  /** Toggle collapsed state */
  onToggleCollapsed?: () => void
  /** Whether this cell is selected */
  isSelected?: boolean
  /** Select this cell */
  onSelect?: () => void
  /** Run this cell */
  onRun?: () => void
  /** Run from this cell (and all below) */
  onRunFromHere?: () => void
  /** Delete this cell */
  onDelete?: () => void
  /** Row count or other status text */
  statusText?: string
  /** Height in pixels */
  height?: number
  /** Callback when height changes via resize handle */
  onHeightChange?: (height: number) => void
  /** Optional content to render in the title bar (between name and controls) */
  titleBarContent?: ReactNode
  /** Cell content */
  children: ReactNode
  /** Props for drag handle (from dnd-kit useSortable) */
  dragHandleProps?: Record<string, unknown>
  /** Whether the cell is currently being dragged */
  isDragging?: boolean
}

export const CellContainer = forwardRef<HTMLDivElement, CellContainerProps>(function CellContainer(
  {
    name,
    type,
    status,
    error,
    collapsed = false,
    onToggleCollapsed,
    isSelected = false,
    onSelect,
    onRun,
    onRunFromHere,
    onDelete,
    statusText,
    height = 300,
    onHeightChange,
    titleBarContent,
    children,
    dragHandleProps,
    isDragging,
    style,
    ...divProps
  },
  ref
) {
  const [showMenu, setShowMenu] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)

  // Get metadata for this cell type
  const meta = getCellTypeMetadata(type)

  // Normalize height - handle legacy 'auto' values from old configs
  const normalizedHeight = typeof height === 'number' ? height : 300
  const currentHeight = useRef(normalizedHeight)

  // Close menu when clicking outside
  useEffect(() => {
    if (!showMenu) return

    const handleClickOutside = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setShowMenu(false)
      }
    }

    document.addEventListener('mousedown', handleClickOutside)
    return () => document.removeEventListener('mousedown', handleClickOutside)
  }, [showMenu])

  const statusColor =
    status === 'loading'
      ? 'text-accent-link'
      : status === 'error'
        ? 'text-accent-error'
        : status === 'blocked'
          ? 'text-theme-text-muted'
          : 'text-theme-text-muted'

  const statusLabel =
    status === 'loading'
      ? (statusText || 'Running...')
      : status === 'error'
        ? 'Error'
        : status === 'blocked'
          ? 'Blocked'
          : statusText || ''

  // Keep currentHeight in sync with prop
  useEffect(() => {
    currentHeight.current = normalizedHeight
  }, [normalizedHeight])

  const handleResize = useCallback(
    (deltaY: number) => {
      const newHeight = Math.max(50, currentHeight.current + deltaY)
      currentHeight.current = newHeight
      onHeightChange?.(newHeight)
    },
    [onHeightChange]
  )

  const contentStyle = normalizedHeight > 0
    ? { height: `${normalizedHeight}px`, overflow: 'auto' as const }
    : { overflow: 'auto' as const }

  // Determine if this cell can run (has an execute method)
  const canRun = !!meta.execute

  return (
    <div
      ref={ref}
      className={`bg-app-panel border-2 rounded-lg overflow-hidden cursor-pointer transition-colors ${
        isSelected
          ? 'border-[var(--selection-border)] bg-[var(--selection-bg)]'
          : 'border-theme-border hover:border-[var(--hover-border)]'
      } ${isDragging ? 'opacity-50' : ''}`}
      style={style}
      onClick={onSelect}
      {...divProps}
    >
      {/* Cell Header */}
      <div
        className={`flex items-center justify-between px-3 py-2 border-b border-theme-border ${
          isSelected ? 'bg-[var(--selection-bg)]' : 'bg-app-card'
        }`}
      >
        <div className="flex items-center gap-2">
          {/* Drag handle */}
          {dragHandleProps && (
            <button
              {...(dragHandleProps as React.ButtonHTMLAttributes<HTMLButtonElement>)}
              className="text-theme-text-muted hover:text-theme-text-primary transition-colors cursor-grab active:cursor-grabbing touch-none"
              onClick={(e) => e.stopPropagation()}
            >
              <GripVertical className="w-4 h-4" />
            </button>
          )}

          {/* Collapse toggle (hidden when no toggle handler, e.g. variable cells with title bar content) */}
          {onToggleCollapsed && (
            <button
              onClick={(e) => {
                e.stopPropagation()
                onToggleCollapsed()
              }}
              className="text-theme-text-muted hover:text-theme-text-primary transition-colors"
            >
              {collapsed ? (
                <ChevronRight className="w-4 h-4" />
              ) : (
                <ChevronDown className="w-4 h-4" />
              )}
            </button>
          )}

          {/* For cells with showTypeBadge=false: show name only. For others: show type badge + name */}
          {!meta.showTypeBadge ? (
            <span className="font-medium text-theme-text-primary">{name}</span>
          ) : (
            <>
              <span className="text-[11px] px-1.5 py-0.5 rounded bg-app-panel text-theme-text-secondary uppercase font-medium">
                {meta.label}
              </span>
              <span className="font-medium text-theme-text-primary">{name}</span>
            </>
          )}
        </div>

        {/* Title bar content (e.g., variable inputs) */}
        {titleBarContent && (
          <div className="flex-1 min-w-0 mx-2">
            {titleBarContent}
          </div>
        )}

        <div className="flex items-center gap-2">
          {/* Status text */}
          {statusLabel && <span className={`text-xs ${statusColor}`}>{statusLabel}</span>}

          {/* Run button (for cells that can execute) */}
          {onRun && canRun && (
            <Button
              variant="ghost"
              size="sm"
              className="h-7 w-7 p-0"
              onClick={(e) => {
                e.stopPropagation()
                onRun()
              }}
              disabled={status === 'loading'}
              title="Run cell"
            >
              {status === 'loading' ? (
                <RotateCcw className="w-3.5 h-3.5 animate-spin" />
              ) : (
                <Play className="w-3.5 h-3.5" />
              )}
            </Button>
          )}

          {/* Menu button */}
          <div className="relative" ref={menuRef}>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 w-7 p-0"
              onClick={(e) => {
                e.stopPropagation()
                setShowMenu(!showMenu)
              }}
            >
              <MoreVertical className="w-3.5 h-3.5" />
            </Button>

            {showMenu && (
              <div className="absolute right-0 top-full mt-1 w-40 bg-app-panel border border-theme-border rounded-md shadow-lg z-50">
                {onRunFromHere && canRun && (
                  <button
                    onClick={(e) => {
                      e.stopPropagation()
                      onRunFromHere()
                      setShowMenu(false)
                    }}
                    className="w-full flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 first:rounded-t-md"
                  >
                    <Play className="w-4 h-4" />
                    Run from here
                  </button>
                )}
                {onDelete && (
                  <button
                    onClick={(e) => {
                      e.stopPropagation()
                      onDelete()
                      setShowMenu(false)
                    }}
                    className="w-full flex items-center gap-2 px-3 py-2 text-sm text-accent-error hover:bg-theme-border/50 last:rounded-b-md"
                  >
                    <Trash2 className="w-4 h-4" />
                    Delete cell
                  </button>
                )}
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Cell Content */}
      {!collapsed && (
        <div className="p-4" style={contentStyle}>
          {status === 'error' && error ? (
            <div className="bg-[var(--error-bg)] border border-accent-error rounded-md p-3 flex items-start gap-3">
              <span className="text-accent-error text-lg">!</span>
              <div>
                <div className="font-medium text-accent-error">Query execution failed</div>
                <div className="text-sm text-theme-text-secondary mt-1">{error}</div>
              </div>
            </div>
          ) : status === 'blocked' ? (
            <div className="bg-app-card border border-dashed border-theme-border rounded-md p-6 text-center text-theme-text-muted">
              Waiting for cell above to succeed
            </div>
          ) : (
            children
          )}
        </div>
      )}

      {/* Resize Handle */}
      {!collapsed && onHeightChange && <ResizeHandle onResize={handleResize} />}
    </div>
  )
})
