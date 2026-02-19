import { useState, useCallback, useRef } from 'react'
import {
  DndContext,
  closestCenter,
  PointerSensor,
  useSensor,
  useSensors,
  DragEndEvent,
  DragOverlay,
  DragStartEvent,
} from '@dnd-kit/core'
import {
  arrayMove,
  SortableContext,
  useSortable,
  horizontalListSortingStrategy,
} from '@dnd-kit/sortable'
import { CSS } from '@dnd-kit/utilities'
import {
  GripVertical,
  Play,
  RotateCcw,
  MoreVertical,
  Trash2,
  ChevronLeft,
  ChevronRight,
  Plus,
  X,
  ArrowLeft,
} from 'lucide-react'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import type { CellTypeMetadata, CellRendererProps, CellEditorProps } from '../cell-registry'
import { getCellTypeMetadata, getCellRenderer, CELL_TYPE_OPTIONS, createDefaultCell } from '../cell-registry'
import type {
  CellConfig,
  CellState,
  CellStatus,
  HorizontalGroupCellConfig,
  VariableValue,
} from '../notebook-types'
import { Button } from '@/components/ui/button'

// =============================================================================
// Types for HorizontalGroupCell props (passed from NotebookRenderer)
// =============================================================================

export interface HorizontalGroupCellProps {
  config: HorizontalGroupCellConfig
  cellStates: Record<string, CellState>
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  selectedChildName: string | null
  onChildSelect: (childName: string | null) => void
  onChildRun: (childName: string) => void
  onConfigChange: (config: HorizontalGroupCellConfig) => void
  onChildDragOut: (childName: string, position: 'before' | 'after') => void
  /** All cell names in notebook (for uniqueness checks) */
  allCellNames: Set<string>
}

// =============================================================================
// Sortable Child Wrapper (horizontal)
// =============================================================================

interface SortableChildProps {
  id: string
  children: (props: {
    dragHandleProps: Record<string, unknown>
    isDragging: boolean
    setNodeRef: (node: HTMLElement | null) => void
    style: React.CSSProperties
  }) => React.ReactNode
}

function SortableChild({ id, children }: SortableChildProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id })
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
  }
  return <>{children({ dragHandleProps: { ...attributes, ...listeners }, isDragging, setNodeRef, style })}</>
}

// =============================================================================
// Compact Child Header
// =============================================================================

interface ChildCellHeaderProps {
  name: string
  type: CellConfig['type']
  status: CellStatus
  statusText?: string
  isSelected: boolean
  onSelect: () => void
  onRun: () => void
  onDelete: () => void
  dragHandleProps?: Record<string, unknown>
}

function ChildCellHeader({
  name,
  type,
  status,
  statusText,
  isSelected,
  onSelect,
  onRun,
  onDelete,
  dragHandleProps,
}: ChildCellHeaderProps) {
  const meta = getCellTypeMetadata(type)
  const canRun = !!meta.execute

  const statusColor =
    status === 'loading'
      ? 'text-accent-link'
      : status === 'error'
        ? 'text-accent-error'
        : 'text-theme-text-muted'

  const statusLabel =
    status === 'loading'
      ? 'Running...'
      : status === 'error'
        ? 'Error'
        : status === 'blocked'
          ? 'Blocked'
          : statusText || ''

  return (
    <div
      className={`flex items-center justify-between px-2 py-1.5 border-b cursor-pointer ${
        isSelected
          ? 'border-[var(--selection-border)] bg-[var(--selection-bg)]'
          : 'border-theme-border bg-app-card'
      }`}
      onClick={(e) => {
        e.stopPropagation()
        onSelect()
      }}
    >
      <div className="flex items-center gap-1.5 min-w-0">
        {dragHandleProps && (
          <button
            {...(dragHandleProps as React.ButtonHTMLAttributes<HTMLButtonElement>)}
            className="text-theme-text-muted hover:text-theme-text-primary transition-colors cursor-grab active:cursor-grabbing touch-none"
            onClick={(e) => e.stopPropagation()}
          >
            <GripVertical className="w-3.5 h-3.5" />
          </button>
        )}
        {meta.showTypeBadge && (
          <span className="text-[10px] px-1 py-0.5 rounded bg-app-panel text-theme-text-secondary uppercase font-medium shrink-0">
            {meta.label}
          </span>
        )}
        <span className="text-sm font-medium text-theme-text-primary truncate">{name}</span>
      </div>

      <div className="flex items-center gap-1 shrink-0">
        {statusLabel && <span className={`text-[10px] ${statusColor}`}>{statusLabel}</span>}
        {canRun && (
          <button
            className="p-0.5 text-theme-text-muted hover:text-theme-text-primary transition-colors"
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
                className="flex items-center gap-2 px-3 py-2 text-sm text-accent-error hover:bg-theme-border/50 cursor-pointer outline-none rounded-md"
                onSelect={() => onDelete()}
              >
                <Trash2 className="w-3.5 h-3.5" />
                Remove from group
              </DropdownMenu.Item>
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
      </div>
    </div>
  )
}

// =============================================================================
// Renderer Component
// =============================================================================

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

// Vertical distance (px) pointer must travel beyond container to trigger drag-out
const DRAG_OUT_THRESHOLD = 30

export function HorizontalGroupCell({
  config,
  cellStates,
  variables,
  timeRange,
  selectedChildName,
  onChildSelect,
  onChildRun,
  onConfigChange,
  onChildDragOut,
}: HorizontalGroupCellProps) {
  const [activeDragId, setActiveDragId] = useState<string | null>(null)
  const containerRef = useRef<HTMLDivElement>(null)
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } })
  )

  const handleDragStart = useCallback((event: DragStartEvent) => {
    setActiveDragId(event.active.id as string)
  }, [])

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      setActiveDragId(null)
      const { active, over } = event

      // Check if pointer ended outside the container → extract from group
      if (containerRef.current) {
        const rect = containerRef.current.getBoundingClientRect()
        const pointerY = (event.activatorEvent as PointerEvent).clientY + event.delta.y
        if (pointerY < rect.top - DRAG_OUT_THRESHOLD) {
          onChildDragOut(active.id as string, 'before')
          return
        }
        if (pointerY > rect.bottom + DRAG_OUT_THRESHOLD) {
          onChildDragOut(active.id as string, 'after')
          return
        }
      }

      if (!over || active.id === over.id) return

      const oldIndex = config.children.findIndex((c) => c.name === active.id)
      const newIndex = config.children.findIndex((c) => c.name === over.id)
      if (oldIndex === -1 || newIndex === -1) return

      const newChildren = arrayMove(config.children, oldIndex, newIndex)
      onConfigChange({ ...config, children: newChildren })
    },
    [config, onConfigChange, onChildDragOut]
  )

  const handleDeleteChild = useCallback(
    (childName: string) => {
      const newChildren = config.children.filter((c) => c.name !== childName)
      onConfigChange({ ...config, children: newChildren })
      if (selectedChildName === childName) {
        onChildSelect(null)
      }
    },
    [config, onConfigChange, selectedChildName, onChildSelect]
  )

  if (config.children.length === 0) {
    return (
      <div className="flex items-center justify-center p-8 text-theme-text-muted border border-dashed border-theme-border rounded-md">
        <span className="text-sm">Add cells to this group from the editor panel</span>
      </div>
    )
  }

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragStart={handleDragStart}
      onDragEnd={handleDragEnd}
    >
      <SortableContext items={config.children.map((c) => c.name)} strategy={horizontalListSortingStrategy}>
        <div ref={containerRef} className="flex gap-2 h-full">
          {config.children.map((child) => {
            const state = cellStates[child.name] || { status: 'idle' as const, data: [] }
            const meta = getCellTypeMetadata(child.type)
            const CellRenderer = getCellRenderer(child.type)
            const rendererProps = meta.getRendererProps(child, state)

            // Status text for child header
            let statusText: string | undefined
            if (state.data && state.data.length > 0) {
              const totalRows = state.data.reduce((sum, t) => sum + t.numRows, 0)
              const totalBytes = state.data.reduce(
                (sum, t) => sum + t.batches.reduce((s: number, b) => s + b.data.byteLength, 0), 0
              )
              statusText = `${totalRows.toLocaleString()} rows (${formatBytes(totalBytes)})`
            }

            return (
              <SortableChild key={child.name} id={child.name}>
                {({ dragHandleProps, isDragging, setNodeRef, style }) => (
                  <div
                    ref={setNodeRef}
                    style={style}
                    className={`flex-1 min-w-0 border rounded-md overflow-hidden flex flex-col ${
                      selectedChildName === child.name
                        ? 'border-[var(--selection-border)]'
                        : 'border-theme-border'
                    } ${isDragging ? 'opacity-50' : ''}`}
                  >
                    <ChildCellHeader
                      name={child.name}
                      type={child.type}
                      status={state.status}
                      statusText={statusText}
                      isSelected={selectedChildName === child.name}
                      onSelect={() => onChildSelect(child.name)}
                      onRun={() => onChildRun(child.name)}
                      onDelete={() => handleDeleteChild(child.name)}
                      dragHandleProps={dragHandleProps}
                    />
                    <div className="flex-1 overflow-auto p-2">
                      {state.status === 'error' && state.error ? (
                        <div className="bg-[var(--error-bg)] border border-accent-error rounded-md p-2 text-xs">
                          <span className="text-accent-error font-medium">Error: </span>
                          <span className="text-theme-text-secondary">{state.error}</span>
                        </div>
                      ) : state.status === 'blocked' ? (
                        <div className="text-center text-theme-text-muted text-xs p-4">
                          Waiting for cell above to succeed
                        </div>
                      ) : (
                        <CellRenderer
                          name={child.name}
                          data={state.data || []}
                          status={state.status}
                          error={state.error}
                          timeRange={timeRange}
                          variables={variables}
                          isEditing={false}
                          onRun={() => onChildRun(child.name)}
                          onSqlChange={() => {}}
                          onOptionsChange={() => {}}
                          {...rendererProps}
                        />
                      )}
                    </div>
                  </div>
                )}
              </SortableChild>
            )
          })}
        </div>
      </SortableContext>
      <DragOverlay>
        {activeDragId ? (
          <div className="bg-app-panel border-2 border-accent-link rounded-md shadow-xl opacity-90 px-3 py-2">
            <span className="text-sm font-medium text-theme-text-primary">{activeDragId}</span>
          </div>
        ) : null}
      </DragOverlay>
    </DndContext>
  )
}

// =============================================================================
// Add Child Modal (reuses cell type options, minus 'hg')
// =============================================================================

interface AddChildModalProps {
  isOpen: boolean
  onClose: () => void
  onAdd: (type: CellConfig['type']) => void
}

function AddChildModal({ isOpen, onClose, onAdd }: AddChildModalProps) {
  if (!isOpen) return null

  const options = CELL_TYPE_OPTIONS.filter((o) => o.type !== 'hg')

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={onClose} />
      <div className="relative w-full max-w-sm bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
          <h2 className="text-lg font-medium text-theme-text-primary">Add Child Cell</h2>
          <button
            onClick={onClose}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>
        <div className="p-2">
          {options.map((option) => (
            <button
              key={option.type}
              onClick={() => onAdd(option.type)}
              className="w-full flex items-center gap-3 p-3 rounded-lg hover:bg-app-card transition-colors text-left"
            >
              <div className="w-10 h-10 bg-app-card rounded-lg flex items-center justify-center text-lg font-semibold text-theme-text-secondary">
                {option.icon}
              </div>
              <div>
                <div className="font-medium text-theme-text-primary">{option.name}</div>
                <div className="text-xs text-theme-text-muted">{option.description}</div>
              </div>
            </button>
          ))}
        </div>
      </div>
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

interface HorizontalGroupCellEditorProps {
  config: HorizontalGroupCellConfig
  onChange: (config: CellConfig) => void
  selectedChildName: string | null
  onChildSelect: (childName: string | null) => void
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  allCellNames: Set<string>
  availableColumns?: string[]
  datasourceVariables?: string[]
  defaultDataSource?: string
}

export function HorizontalGroupCellEditor({
  config,
  onChange,
  selectedChildName,
  onChildSelect,
  variables,
  timeRange,
  allCellNames,
  availableColumns,
  datasourceVariables,
  defaultDataSource,
}: HorizontalGroupCellEditorProps) {
  const [showAddChildModal, setShowAddChildModal] = useState(false)

  // If a child is selected, show its editor
  const selectedChild = selectedChildName
    ? config.children.find((c) => c.name === selectedChildName)
    : null

  if (selectedChild) {
    const meta = getCellTypeMetadata(selectedChild.type)
    return (
      <>
        <button
          onClick={() => onChildSelect(null)}
          className="flex items-center gap-1 text-sm text-accent-link hover:underline mb-3"
        >
          <ArrowLeft className="w-3.5 h-3.5" />
          Back to group
        </button>
        <div className="text-xs text-theme-text-muted uppercase font-medium mb-2">
          Editing child: {selectedChild.name}
        </div>
        <meta.EditorComponent
          config={selectedChild}
          onChange={(newConfig) => {
            const newChildren = config.children.map((c) =>
              c.name === selectedChild.name ? newConfig : c
            )
            onChange({ ...config, children: newChildren })
            // If child was renamed, update selection
            if (newConfig.name !== selectedChild.name) {
              onChildSelect(newConfig.name)
            }
          }}
          variables={variables}
          timeRange={timeRange}
          availableColumns={availableColumns}
          datasourceVariables={datasourceVariables}
          defaultDataSource={defaultDataSource}
        />
      </>
    )
  }

  // Group editor: list of children with reorder/remove
  const handleAddChild = (type: CellConfig['type']) => {
    const newCell = createDefaultCell(type, allCellNames)
    const newChildren = [...config.children, newCell]
    onChange({ ...config, children: newChildren })
    setShowAddChildModal(false)
  }

  const handleRemoveChild = (childName: string) => {
    const newChildren = config.children.filter((c) => c.name !== childName)
    onChange({ ...config, children: newChildren })
  }

  const handleMoveChild = (index: number, direction: -1 | 1) => {
    const newIndex = index + direction
    if (newIndex < 0 || newIndex >= config.children.length) return
    const newChildren = arrayMove(config.children, index, newIndex)
    onChange({ ...config, children: newChildren })
  }

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Children ({config.children.length})
        </label>
        {config.children.length === 0 ? (
          <div className="text-sm text-theme-text-muted py-4 text-center border border-dashed border-theme-border rounded-md">
            No children yet
          </div>
        ) : (
          <div className="space-y-1">
            {config.children.map((child, index) => {
              const meta = getCellTypeMetadata(child.type)
              return (
                <div
                  key={child.name}
                  className="flex items-center gap-1.5 px-2 py-1.5 bg-app-card rounded-md group"
                >
                  <span className="text-[10px] px-1 py-0.5 rounded bg-app-panel text-theme-text-secondary uppercase font-medium shrink-0">
                    {meta.icon}
                  </span>
                  <button
                    className="text-sm text-theme-text-primary truncate flex-1 text-left hover:text-accent-link"
                    onClick={() => onChildSelect(child.name)}
                    title={`Edit ${child.name}`}
                  >
                    {child.name}
                  </button>
                  <button
                    onClick={() => handleMoveChild(index, -1)}
                    disabled={index === 0}
                    className="p-0.5 text-theme-text-muted hover:text-theme-text-primary disabled:opacity-30 transition-colors"
                    title="Move left"
                  >
                    <ChevronLeft className="w-3.5 h-3.5" />
                  </button>
                  <button
                    onClick={() => handleMoveChild(index, 1)}
                    disabled={index === config.children.length - 1}
                    className="p-0.5 text-theme-text-muted hover:text-theme-text-primary disabled:opacity-30 transition-colors"
                    title="Move right"
                  >
                    <ChevronRight className="w-3.5 h-3.5" />
                  </button>
                  <button
                    onClick={() => handleRemoveChild(child.name)}
                    className="p-0.5 text-theme-text-muted hover:text-accent-error transition-colors"
                    title="Remove"
                  >
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>
                </div>
              )
            })}
          </div>
        )}
      </div>

      <Button
        variant="outline"
        className="w-full gap-2"
        onClick={() => setShowAddChildModal(true)}
      >
        <Plus className="w-4 h-4" />
        Add Child Cell
      </Button>

      <AddChildModal
        isOpen={showAddChildModal}
        onClose={() => setShowAddChildModal(false)}
        onAdd={handleAddChild}
      />
    </>
  )
}

// Thin wrapper so it matches CellEditorProps shape for the registry
function HorizontalGroupCellEditorWrapper(_props: CellEditorProps) {
  // The actual editor is handled in CellEditor.tsx via HorizontalGroupCellEditor
  // This is a placeholder to satisfy the registry
  return null
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

// eslint-disable-next-line react-refresh/only-export-components
export const hgMetadata: CellTypeMetadata = {
  renderer: function HgPlaceholder(_props: CellRendererProps) {
    // The actual rendering is handled directly by NotebookRenderer
    // which renders HorizontalGroupCell with additional props
    return null
  },
  EditorComponent: HorizontalGroupCellEditorWrapper,

  label: 'Group',
  icon: 'H',
  description: 'Arrange cells side by side in a row',
  showTypeBadge: true,
  defaultHeight: 300,

  canBlockDownstream: false,

  createDefaultConfig: () => ({
    type: 'hg' as const,
    children: [],
  }),

  // No execute method - hg cells don't execute (children do)

  getRendererProps: () => ({}),
}
