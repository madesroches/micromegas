/**
 * Per-child pane + sortable wrapper for HorizontalGroupCell.
 *
 * Extracted from HorizontalGroupCell.tsx (#1089). `SortableChild` is the dnd-kit
 * sortable render-prop wrapper; `HgChildPane` renders one child cell (title bar,
 * run/menu controls, and the nested cell renderer). Both are presentational —
 * the parent owns drag-and-drop orchestration and child state.
 */
import { useRef, useCallback } from 'react'
import { useSortable } from '@dnd-kit/sortable'
import { CSS } from '@dnd-kit/utilities'
import {
  Download,
  GripVertical,
  Pencil,
  Play,
  RotateCcw,
  MoreVertical,
  Trash2,
} from 'lucide-react'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import type { CellRendererProps } from '../cell-registry'
import { getCellTypeMetadata, getCellRenderer } from '../cell-registry'
import { useFadeOnIdle } from '@/hooks/useFadeOnIdle'
import type { CellConfig, CellState } from '../notebook-types'
import { buildStatusText } from '../notebook-cell-view'

interface SortableChildProps {
  id: string
  children: (props: {
    dragHandleProps: Record<string, unknown>
    isDragging: boolean
    setNodeRef: (node: HTMLElement | null) => void
    style: React.CSSProperties
  }) => React.ReactNode
}

export function SortableChild({ id, children }: SortableChildProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id })
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
  }
  return <>{children({ dragHandleProps: { ...attributes, ...listeners }, isDragging, setNodeRef, style })}</>
}

interface HgChildPaneProps {
  child: CellConfig
  state: CellState
  commonProps: CellRendererProps
  isSelected: boolean
  onSelect: () => void
  onRun: () => void
  onDownloadCsv?: () => void
  onDeleteChild: () => void
  dragHandleProps: Record<string, unknown>
  isDragging: boolean
  setNodeRef: (node: HTMLElement | null) => void
  style: React.CSSProperties
  showDivider: boolean
  onChildRef?: (name: string, el: HTMLElement | null) => void
}

export function HgChildPane({
  child, state, commonProps,
  isSelected,
  onSelect, onRun, onDownloadCsv, onDeleteChild,
  dragHandleProps, isDragging, setNodeRef, style, showDivider, onChildRef,
}: HgChildPaneProps) {
  const setNodeRefStable = useRef(setNodeRef)
  setNodeRefStable.current = setNodeRef

  const combinedRef = useCallback((el: HTMLElement | null) => {
    setNodeRefStable.current(el)
    onChildRef?.(child.name, el)
  }, [onChildRef, child.name])
  const fadeClass = useFadeOnIdle(state.status)

  const meta = getCellTypeMetadata(child.type)
  const CellRenderer = getCellRenderer(child.type)
  const TitleBarRenderer = meta.titleBarRenderer
  const canRun = !!meta.execute

  const statusText = buildStatusText(child, state)

  const statusColor =
    state.status === 'loading'
      ? 'text-accent-link'
      : state.status === 'error'
        ? 'text-accent-error'
        : 'text-theme-text-muted'

  const statusLabel =
    state.status === 'loading'
      ? (statusText || 'Running...')
      : state.status === 'error'
        ? 'Error'
        : state.status === 'blocked'
          ? 'Blocked'
          : statusText || ''

  return (
    <div
      ref={combinedRef}
      style={style}
      className={`flex-1 min-w-0 flex group/pane overflow-hidden ${isDragging ? 'opacity-50' : ''}`}
    >
      {showDivider && <div className={`w-px shrink-0 bg-theme-border ${fadeClass}`} />}
      <div className={`flex-1 min-w-0 flex flex-col border-l-2 ${
        isSelected ? 'border-l-accent-link' : 'border-l-transparent'
      }`}>
      {/* Pane label */}
      <div
        className={`flex items-center justify-between px-2 py-0.5 cursor-pointer ${
          isSelected ? 'bg-[var(--selection-bg)]' : ''
        }`}
        onMouseDown={(e) => { if (e.detail > 1) e.preventDefault() }}
        onDoubleClick={(e) => {
          e.stopPropagation()
          onSelect()
        }}
      >
        <div className="flex items-center gap-1 min-w-0 flex-1">
          {dragHandleProps && (
            <button
              {...(dragHandleProps as React.ButtonHTMLAttributes<HTMLButtonElement>)}
              className="opacity-0 group-hover/pane:opacity-100 text-theme-text-muted hover:text-theme-text-primary transition-all cursor-grab active:cursor-grabbing touch-none"
              onClick={(e) => e.stopPropagation()}
              onDoubleClick={(e) => e.stopPropagation()}
            >
              <GripVertical className="w-3 h-3" />
            </button>
          )}
          <span className="text-[10px] font-medium text-theme-text-secondary truncate shrink-0">
            {child.name}
          </span>
          {TitleBarRenderer && (
            <div className="flex-1 min-w-0" onClick={(e) => e.stopPropagation()}>
              <TitleBarRenderer {...commonProps} />
            </div>
          )}
          {statusLabel && (
            <>
              <span className={`text-[10px] text-theme-border ${fadeClass}`}>&middot;</span>
              <span className={`text-[10px] ${statusColor} ${fadeClass}`}>{statusLabel}</span>
            </>
          )}
        </div>

        <div className="flex items-center gap-0.5 shrink-0 opacity-0 group-hover/pane:opacity-100 transition-opacity" onDoubleClick={(e) => e.stopPropagation()}>
          {canRun && (
            <button
              className="p-0.5 text-theme-text-muted hover:text-theme-text-primary transition-colors"
              onClick={(e) => {
                e.stopPropagation()
                onRun()
              }}
              disabled={state.status === 'loading'}
              title="Run cell"
            >
              {state.status === 'loading' ? (
                <RotateCcw className="w-3 h-3 animate-spin" />
              ) : (
                <Play className="w-3 h-3" />
              )}
            </button>
          )}
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button
                className="p-0.5 text-theme-text-muted hover:text-theme-text-primary transition-colors"
                onClick={(e) => e.stopPropagation()}
              >
                <MoreVertical className="w-3 h-3" />
              </button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content
                align="end"
                sideOffset={4}
                className="w-40 bg-app-panel border border-theme-border rounded-md shadow-lg z-50"
                onClick={(e) => e.stopPropagation()}
              >
                <DropdownMenu.Item
                  className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none rounded-t-md"
                  onSelect={onSelect}
                >
                  <Pencil className="w-3.5 h-3.5" />
                  Edit cell
                </DropdownMenu.Item>
                {onDownloadCsv && (
                  <DropdownMenu.Item
                    className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none"
                    onSelect={() => onDownloadCsv()}
                  >
                    <Download className="w-3.5 h-3.5" />
                    Download CSV
                  </DropdownMenu.Item>
                )}
                <DropdownMenu.Item
                  className="flex items-center gap-2 px-3 py-2 text-sm text-accent-error hover:bg-theme-border/50 cursor-pointer outline-none rounded-b-md"
                  onSelect={onDeleteChild}
                >
                  <Trash2 className="w-3.5 h-3.5" />
                  Remove from group
                </DropdownMenu.Item>
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        </div>
      </div>

      {/* Content */}
      <div
        className="flex-1 overflow-auto px-1 pb-1"
        onMouseDown={(e) => { if (e.detail > 1) e.preventDefault() }}
        onDoubleClick={(e) => {
          e.stopPropagation();
          onSelect();
        }}
      >
        {state.status === 'error' && state.error ? (
          <div className="bg-[var(--error-bg)] border border-accent-error rounded-md p-2 text-xs">
            <span className="text-accent-error font-medium">Error: </span>
            <span className="text-theme-text-secondary">{state.error}</span>
          </div>
        ) : state.status === 'blocked' ? (
          <div className="text-center text-theme-text-muted text-xs p-4">
            {state.error || 'Waiting for cell above to succeed'}
          </div>
        ) : (
          <CellRenderer {...commonProps} />
        )}
      </div>
      </div>
    </div>
  )
}
