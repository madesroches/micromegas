import { ReactNode, useRef, useEffect, forwardRef, HTMLAttributes, useCallback } from 'react'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import { ChevronDown, ChevronRight, Copy, Download, Pencil, Play, RotateCcw, MoreVertical, Trash2, GripVertical, Zap } from 'lucide-react'
import { CellType, CellStatus, getCellTypeMetadata } from '@/lib/screen-renderers/cell-registry'
import { Button } from '@/components/ui/button'
import { ResizeHandle } from '@/components/ResizeHandle'
import { useFadeOnIdle } from '@/hooks/useFadeOnIdle'

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
  /** Whether this cell auto-runs from here when upstream variables change */
  autoRunFromHere?: boolean
  /** Toggle auto-run from here */
  onToggleAutoRunFromHere?: () => void
  /** Duplicate this cell */
  onDuplicate?: () => void
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
  /** Override whether the run button is shown (defaults to cell type's execute capability) */
  canRun?: boolean
  /** Download cell data as CSV */
  onDownloadCsv?: () => void
  /** Child cell names (for collapsed HG groups to show inline) */
  childNames?: string[]
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
    autoRunFromHere,
    onToggleAutoRunFromHere,
    onDownloadCsv,
    onDuplicate,
    onDelete,
    statusText,
    height = 300,
    onHeightChange,
    titleBarContent,
    children,
    dragHandleProps,
    isDragging,
    canRun: canRunProp,
    childNames,
    style,
    ...divProps
  },
  ref
) {
  const meta = getCellTypeMetadata(type)
  const canRun = canRunProp ?? !!meta.execute
  const isGroup = type === 'hg'

  const fadeClass = useFadeOnIdle(status)

  // Normalize height - handle legacy 'auto' values from old configs
  const normalizedHeight = typeof height === 'number' ? height : 300
  const currentHeight = useRef(normalizedHeight)

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

  // --- Shared UI elements ---

  const gripHandle = dragHandleProps ? (
    <button
      {...(dragHandleProps as React.ButtonHTMLAttributes<HTMLButtonElement>)}
      className="opacity-0 group-hover/cell:opacity-100 text-theme-text-muted hover:text-theme-text-primary transition-all cursor-grab active:cursor-grabbing touch-none"
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      <GripVertical className="w-3.5 h-3.5" />
    </button>
  ) : null

  const collapseToggle = onToggleCollapsed ? (
    <button
      onClick={(e) => {
        e.stopPropagation()
        onToggleCollapsed()
      }}
      onDoubleClick={(e) => e.stopPropagation()}
      className={`text-theme-text-muted hover:text-theme-text-primary transition-colors ${fadeClass}`}
    >
      {collapsed ? (
        <ChevronRight className="w-3.5 h-3.5" />
      ) : (
        <ChevronDown className="w-3.5 h-3.5" />
      )}
    </button>
  ) : null

  const hoverControls = (
    <div className="flex items-center gap-1 opacity-0 group-hover/cell:opacity-100 shrink-0 transition-opacity" onDoubleClick={(e) => e.stopPropagation()}>
      {onRun && canRun && (
        <Button
          variant="ghost"
          size="sm"
          className="h-6 w-6 p-0"
          onClick={(e) => {
            e.stopPropagation()
            onRun()
          }}
          disabled={status === 'loading'}
          title="Run cell"
        >
          {status === 'loading' ? (
            <RotateCcw className="w-3 h-3 animate-spin" />
          ) : (
            <Play className="w-3 h-3" />
          )}
        </Button>
      )}

      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <Button
            variant="ghost"
            size="sm"
            className="h-6 w-6 p-0"
            onClick={(e) => e.stopPropagation()}
          >
            <MoreVertical className="w-3 h-3" />
          </Button>
        </DropdownMenu.Trigger>

        <DropdownMenu.Portal>
          <DropdownMenu.Content
            align="end"
            sideOffset={4}
            className="w-48 bg-app-panel border border-theme-border rounded-md shadow-lg z-50"
            onClick={(e) => e.stopPropagation()}
          >
            {onSelect && (
              <DropdownMenu.Item
                className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none first:rounded-t-md"
                onSelect={() => onSelect()}
              >
                <Pencil className="w-4 h-4" />
                Edit cell
              </DropdownMenu.Item>
            )}
            {onRunFromHere && canRun && (
              <DropdownMenu.Item
                className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none first:rounded-t-md"
                onSelect={() => onRunFromHere()}
              >
                <Play className="w-4 h-4" />
                Run from here
              </DropdownMenu.Item>
            )}
            {onToggleAutoRunFromHere && canRun && (
              <DropdownMenu.Item
                className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none"
                onSelect={() => onToggleAutoRunFromHere()}
              >
                <Zap className={`w-4 h-4 ${autoRunFromHere ? 'text-accent-link' : ''}`} />
                {autoRunFromHere ? 'Disable auto-run' : 'Auto-run from here'}
              </DropdownMenu.Item>
            )}
            {onDownloadCsv && (
              <DropdownMenu.Item
                className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none"
                onSelect={() => onDownloadCsv()}
              >
                <Download className="w-4 h-4" />
                Download CSV
              </DropdownMenu.Item>
            )}
            {onDuplicate && (
              <DropdownMenu.Item
                className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none"
                onSelect={() => onDuplicate()}
              >
                <Copy className="w-4 h-4" />
                Duplicate cell
              </DropdownMenu.Item>
            )}
            {onDelete && (
              <DropdownMenu.Item
                className="flex items-center gap-2 px-3 py-2 text-sm text-accent-error hover:bg-theme-border/50 cursor-pointer outline-none last:rounded-b-md"
                onSelect={() => onDelete()}
              >
                <Trash2 className="w-4 h-4" />
                Delete cell
              </DropdownMenu.Item>
            )}
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </div>
  )

  const renderContent = () => {
    if (status === 'error' && error) {
      return (
        <div className="bg-[var(--error-bg)] border border-accent-error rounded-md p-3 flex items-start gap-3">
          <span className="text-accent-error text-lg">!</span>
          <div>
            <div className="font-medium text-accent-error">Query execution failed</div>
            <div className="text-sm text-theme-text-secondary mt-1">{error}</div>
          </div>
        </div>
      )
    }
    if (status === 'blocked') {
      return (
        <div className="bg-app-card border border-dashed border-theme-border rounded-md p-6 text-center text-theme-text-muted">
          Waiting for cell above to succeed
        </div>
      )
    }
    return children
  }

  // ===== Variant 1: Collapsed regular cell (not group, not variable) =====
  if (collapsed && !isGroup && !titleBarContent) {
    return (
      <div
        ref={ref}
        className={`flex items-center gap-1.5 py-0.5 px-1.5 rounded cursor-pointer group/cell transition-colors border-l-2 ${
          isSelected
            ? 'bg-[var(--selection-bg)] border-l-accent-link'
            : 'border-l-transparent bg-app-panel/30 hover:bg-app-panel/50'
        } ${isDragging ? 'opacity-50' : ''}`}
        style={style}
        onMouseDown={(e) => { if (e.detail > 1) e.preventDefault() }}
        onDoubleClick={() => onSelect?.()}
        {...divProps}
      >
        {gripHandle}
        {collapseToggle}
        <span className="text-[11px] font-medium text-theme-text-secondary">{name}</span>
        {autoRunFromHere && (
          <span className="text-accent-link" title="Auto-run from here">
            <Zap className="w-3 h-3" />
          </span>
        )}
        {status === 'loading' && (
          <RotateCcw className="w-3 h-3 text-accent-link animate-spin shrink-0" />
        )}
        {statusLabel && (
          <>
            <span className={`text-[10px] text-theme-border ${fadeClass}`}>&middot;</span>
            <span className={`text-[10px] ${statusColor} ${fadeClass}`}>{statusLabel}</span>
          </>
        )}
        <div className="ml-auto">{hoverControls}</div>
      </div>
    )
  }

  // ===== Variant 2: Variable cell (with titleBarContent) =====
  if (titleBarContent) {
    return (
      <div
        ref={ref}
        className={`group/cell cursor-pointer transition-colors ${isDragging ? 'opacity-50' : ''}`}
        style={style}
        onMouseDown={(e) => { if (e.detail > 1) e.preventDefault() }}
        onDoubleClick={() => onSelect?.()}
        {...divProps}
      >
        <div className={`flex items-center gap-2 py-0.5 px-1.5 rounded transition-colors border-l-2 ${
          isSelected ? 'bg-[var(--selection-bg)] border-l-accent-link' : 'border-l-transparent hover:bg-app-panel/30'
        }`}>
          {gripHandle}
          <span className="text-[11px] font-medium text-theme-text-secondary shrink-0">{name}</span>
          <div className="flex-1 min-w-0" onClick={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
            {titleBarContent}
          </div>
          {statusLabel && <span className={`text-[10px] ${statusColor} shrink-0 ${fadeClass}`}>{statusLabel}</span>}
          {hoverControls}
        </div>
        {!collapsed && (
          <div className="px-1 pb-1">{renderContent()}</div>
        )}
      </div>
    )
  }

  // ===== Variant 3: Group (HG) or regular expanded cell =====
  return (
    <div
      ref={ref}
      className={`group/cell cursor-pointer transition-colors ${
        !isGroup ? `border-l-2 ${isSelected ? 'border-l-accent-link' : 'border-l-transparent'}` : ''
      } ${isDragging ? 'opacity-50' : ''}`}
      style={style}
      onMouseDown={(e) => { if (e.detail > 1) e.preventDefault() }}
      onDoubleClick={() => onSelect?.()}
      {...divProps}
    >
      {/* Header */}
      {isGroup ? (
        // Section divider for groups
        <div className={`flex items-center gap-1.5 pt-1 pb-0.5 px-1 border-l-2 ${
          isSelected ? 'border-l-accent-link' : 'border-l-transparent'
        }`}>
          {gripHandle}
          {collapseToggle}
          <span className={`text-[11px] font-semibold text-theme-text-muted uppercase tracking-wide whitespace-nowrap ${fadeClass}`}>
            {name}
          </span>
          {collapsed && childNames && childNames.length > 0 && (
            <span className={`text-[10px] text-theme-text-muted truncate min-w-0 ${fadeClass}`}>
              {childNames.join(', ')}
            </span>
          )}
          {status === 'loading' && (
            <RotateCcw className="w-3 h-3 text-accent-link animate-spin shrink-0" />
          )}
          {statusLabel && (
            <>
              <span className={`text-[10px] text-theme-border ${fadeClass}`}>&middot;</span>
              <span className={`text-[10px] ${statusColor} whitespace-nowrap ${fadeClass}`}>
                {statusLabel}
              </span>
            </>
          )}
          <span className={`flex-1 h-px bg-theme-border ${fadeClass}`} />
          {hoverControls}
        </div>
      ) : (
        // Pane label for regular expanded cells
        <div className={`flex items-center gap-1.5 px-1.5 py-0.5 ${
          isSelected ? 'bg-[var(--selection-bg)]' : ''
        }`}>
          {gripHandle}
          {collapseToggle}
          {!meta.showTypeBadge ? (
            <span className="text-[11px] font-medium text-theme-text-secondary">{name}</span>
          ) : (
            <>
              <span className="text-[10px] px-1 py-0.5 rounded bg-app-panel text-theme-text-secondary uppercase font-medium">
                {meta.label}
              </span>
              <span className="text-[11px] font-medium text-theme-text-secondary">{name}</span>
            </>
          )}
          {autoRunFromHere && (
            <span className="text-accent-link" title="Auto-run from here">
              <Zap className="w-3 h-3" />
            </span>
          )}
          {status === 'loading' && (
            <RotateCcw className="w-3 h-3 text-accent-link animate-spin shrink-0" />
          )}
          {statusLabel && (
            <>
              <span className={`text-[10px] text-theme-border ${fadeClass}`}>&middot;</span>
              <span className={`text-[10px] ${statusColor} ${fadeClass}`}>{statusLabel}</span>
            </>
          )}
          <div className="ml-auto">{hoverControls}</div>
        </div>
      )}

      {/* Content */}
      {!collapsed && (
        <div className={isGroup ? '' : 'px-1 pb-1'} style={contentStyle}>
          {renderContent()}
        </div>
      )}

      {/* Resize Handle */}
      {!collapsed && onHeightChange && (
        <div className={fadeClass}>
          <ResizeHandle onResize={handleResize} />
        </div>
      )}
    </div>
  )
})
