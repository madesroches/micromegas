import { useState, useCallback, useRef, useEffect } from 'react'
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
  horizontalListSortingStrategy,
} from '@dnd-kit/sortable'
import {
  Play,
  Trash2,
  ChevronLeft,
  ChevronRight,
  Plus,
  ArrowLeft,
  Group,
} from 'lucide-react'
import type { CellTypeMetadata, CellRendererProps, CellEditorProps } from '../cell-registry'
import { getCellTypeMetadata, createDefaultCell } from '../cell-registry'
import { AddCellModal } from '../shared'
import type {
  CellConfig,
  CellState,
  HorizontalGroupCellConfig,
  VariableValue,
} from '../notebook-types'
import { resolveCellDataSource, shouldShowDataSource, validateCellName, sanitizeCellName } from '../notebook-utils'
import { buildCellRendererProps } from '../notebook-cell-view'
import { Button } from '@/components/ui/button'
import { DataSourceField } from '@/components/DataSourceSelector'
import { arrowTableToCsv, triggerCsvDownload } from './arrow-to-csv'
import { SortableChild, HgChildPane } from './HgChildPane'

// =============================================================================
// Types for HorizontalGroupCell props (passed from NotebookRenderer)
// =============================================================================

export interface HorizontalGroupCellProps {
  config: HorizontalGroupCellConfig
  cellStates: Record<string, CellState>
  variables: Record<string, VariableValue>
  variableValues: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, import('apache-arrow').Table>
  cellSelections: Record<string, Record<string, unknown>>
  selectedChildName: string | null
  onChildSelect: (childName: string | null) => void
  onChildRun: (childName: string) => void
  onVariableValueChange: (cellName: string, value: VariableValue) => void
  onConfigChange: (config: HorizontalGroupCellConfig) => void
  onChildDragOut: (childName: string, position: 'before' | 'after') => void
  onTimeRangeSelect?: (from: Date, to: Date) => void
  onSelectionChange?: (cellName: string, selectedRow: Record<string, unknown> | null) => void
  defaultDataSource?: string
  /** All cell names in notebook (for uniqueness checks) */
  allCellNames: Set<string>
  /** Callback to register a child cell's DOM element for keyboard navigation */
  onChildRef?: (name: string, el: HTMLElement | null) => void
}

// =============================================================================
// Renderer Component
// =============================================================================

// Vertical distance (px) pointer must travel beyond container to trigger drag-out
const DRAG_OUT_THRESHOLD = 30

export function HorizontalGroupCell({
  config,
  cellStates,
  variables,
  variableValues,
  timeRange,
  cellResults,
  cellSelections,
  selectedChildName,
  onChildSelect,
  onChildRun,
  onVariableValueChange,
  onConfigChange,
  onChildDragOut,
  onTimeRangeSelect,
  onSelectionChange,
  defaultDataSource,
  onChildRef,
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

  const updateChildConfig = useCallback(
    (childName: string, updates: Partial<CellConfig>) => {
      const newChildren = config.children.map((c) =>
        c.name === childName ? { ...c, ...updates } : c,
      ) as CellConfig[]
      onConfigChange({ ...config, children: newChildren })
    },
    [config, onConfigChange],
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
        <div ref={containerRef} className="flex gap-px h-full">
          {config.children.map((child, index) => {
            const state = cellStates[child.name] || { status: 'idle' as const, data: [] }

            const childMeta = getCellTypeMetadata(child.type)
            const childSelectionMode =
              ((child as import('../notebook-types').QueryCellConfig).options?.selectionMode as 'none' | 'single' | undefined)
              ?? childMeta.defaultSelectionMode
              ?? 'none'

            const commonProps = buildCellRendererProps(child, state,
              {
                availableVariables: variables,
                allVariableValues: variableValues,
                timeRange,
                isEditing: false,
                dataSource: resolveCellDataSource(child, variables, defaultDataSource),
                cellResults,
                cellSelections,
              },
              {
                onRun: () => onChildRun(child.name),
                onSqlChange: (sql) => updateChildConfig(child.name, { sql }),
                onOptionsChange: (options) => updateChildConfig(child.name, { options }),
                onContentChange: (content) => updateChildConfig(child.name, { content }),
                onValueChange: (value) => onVariableValueChange(child.name, value),
                onTimeRangeSelect,
              },
            )

            // Attach selection props for table cells
            if (childSelectionMode === 'single' && onSelectionChange) {
              commonProps.selectionMode = childSelectionMode
              commonProps.onSelectionChange = (row) => onSelectionChange(child.name, row)
            }

            return (
              <SortableChild key={child.name} id={child.name}>
                {({ dragHandleProps, isDragging, setNodeRef, style }) => (
                  <HgChildPane
                    child={child}
                    state={state}
                    commonProps={commonProps}
                    isSelected={selectedChildName === child.name}
                    onSelect={() => onChildSelect(child.name)}
                    onRun={() => onChildRun(child.name)}
                    onDownloadCsv={
                      state.data.length > 0 && state.data[0].numRows > 0
                        ? () => {
                            const csv = arrowTableToCsv(state.data[0])
                            triggerCsvDownload(csv, `${child.name}.csv`)
                          }
                        : undefined
                    }
                    onDeleteChild={() => handleDeleteChild(child.name)}
                    dragHandleProps={dragHandleProps}
                    isDragging={isDragging}
                    setNodeRef={setNodeRef}
                    style={style}
                    showDivider={index > 0}
                    onChildRef={onChildRef}
                  />
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
// Child Editor View (with name field)
// =============================================================================

interface ChildEditorViewProps {
  child: CellConfig
  config: HorizontalGroupCellConfig
  onChange: (config: CellConfig) => void
  onChildSelect: (childName: string | null) => void
  onRun?: () => void
  allCellNames: Set<string>
  defaultDataSource?: string
  datasourceVariables?: string[]
  showNotebookOption?: boolean
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, import('apache-arrow').Table>
  cellSelections: Record<string, Record<string, unknown>>
  availableColumns?: string[]
  meta: CellTypeMetadata
}

function ChildEditorView({
  child,
  config,
  onChange,
  onChildSelect,
  onRun,
  allCellNames,
  defaultDataSource,
  datasourceVariables,
  showNotebookOption,
  variables,
  timeRange,
  cellResults,
  cellSelections,
  availableColumns,
  meta,
}: ChildEditorViewProps) {
  const [editedName, setEditedName] = useState(child.name)
  const [nameError, setNameError] = useState<string | null>(null)

  useEffect(() => {
    setEditedName(child.name)
    setNameError(null)
  }, [child.name])

  const handleNameChange = useCallback(
    (value: string) => {
      setEditedName(value)
      const error = validateCellName(value, allCellNames, child.name)
      if (error) {
        setNameError(error)
        return
      }
      setNameError(null)
      const sanitized = sanitizeCellName(value)
      const newChildren = config.children.map((c) =>
        c.name === child.name ? { ...c, name: sanitized } : c,
      )
      onChange({ ...config, children: newChildren })
      onChildSelect(sanitized)
    },
    [child.name, allCellNames, config, onChange, onChildSelect],
  )

  return (
    <>
      <button
        onClick={() => onChildSelect(null)}
        className="flex items-center gap-1 text-sm text-accent-link hover:underline mb-3"
      >
        <ArrowLeft className="w-3.5 h-3.5" />
        Back to group
      </button>
      {/* Child Name */}
      <div className="mb-3">
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Cell Name
        </label>
        <input
          type="text"
          value={editedName}
          onChange={(e) => handleNameChange(e.target.value)}
          className={`w-full px-3 py-2 bg-app-card border rounded-md text-theme-text-primary text-sm focus:outline-none ${
            nameError
              ? 'border-accent-error focus:border-accent-error'
              : 'border-theme-border focus:border-accent-link'
          }`}
        />
        {nameError && (
          <p className="mt-1 text-xs text-accent-error">{nameError}</p>
        )}
      </div>
      {shouldShowDataSource(child.type) && (
        <DataSourceField
          value={('dataSource' in child ? child.dataSource : undefined) || defaultDataSource || ''}
          onChange={(ds) => {
            const newChildren = config.children.map((c) =>
              c.name === child.name ? { ...c, dataSource: ds } : c,
            )
            onChange({ ...config, children: newChildren })
          }}
          datasourceVariables={datasourceVariables}
          showNotebookOption={showNotebookOption}
        />
      )}
      <meta.EditorComponent
        config={child}
        onChange={(newConfig) => {
          const newChildren = config.children.map((c) =>
            c.name === child.name ? newConfig : c,
          )
          onChange({ ...config, children: newChildren })
          if (newConfig.name !== child.name) {
            onChildSelect(newConfig.name)
          }
        }}
        variables={variables}
        timeRange={timeRange}
        availableColumns={availableColumns}
        datasourceVariables={datasourceVariables}
        defaultDataSource={defaultDataSource}
        onRun={onRun}
        cellResults={cellResults}
        cellSelections={cellSelections}
      />
      {onRun && !!meta.execute && (
        <Button onClick={onRun} className="w-full gap-2">
          <Play className="w-4 h-4" />
          Run
        </Button>
      )}
    </>
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
  onChildRun?: (childName: string) => void
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, import('apache-arrow').Table>
  cellSelections: Record<string, Record<string, unknown>>
  allCellNames: Set<string>
  availableColumns?: string[]
  datasourceVariables?: string[]
  defaultDataSource?: string
  showNotebookOption?: boolean
}

export function HorizontalGroupCellEditor({
  config,
  onChange,
  selectedChildName,
  onChildSelect,
  onChildRun,
  variables,
  timeRange,
  cellResults,
  cellSelections,
  allCellNames,
  availableColumns,
  datasourceVariables,
  defaultDataSource,
  showNotebookOption,
}: HorizontalGroupCellEditorProps) {
  const [showAddChildModal, setShowAddChildModal] = useState(false)

  // If a child is selected, show its editor
  const selectedChild = selectedChildName
    ? config.children.find((c) => c.name === selectedChildName)
    : null

  if (selectedChild) {
    const meta = getCellTypeMetadata(selectedChild.type)
    return (
      <ChildEditorView
        child={selectedChild}
        config={config}
        onChange={onChange}
        onChildSelect={onChildSelect}
        onRun={onChildRun ? () => onChildRun(selectedChild.name) : undefined}
        allCellNames={allCellNames}
        defaultDataSource={defaultDataSource}
        datasourceVariables={datasourceVariables}
        showNotebookOption={showNotebookOption}
        variables={variables}
        timeRange={timeRange}
        cellResults={cellResults}
        cellSelections={cellSelections}
        availableColumns={availableColumns}
        meta={meta}
      />
    )
  }

  // Group editor: list of children with reorder/remove
  const handleAddChild = (type: CellConfig['type']) => {
    const newCell = createDefaultCell(type, allCellNames, defaultDataSource)
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
                  <span className="inline-flex items-center justify-center text-[10px] px-1 py-0.5 rounded bg-app-panel text-theme-text-secondary uppercase font-medium shrink-0 [&_svg]:w-3 [&_svg]:h-3">
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

      <AddCellModal
        isOpen={showAddChildModal}
        onClose={() => setShowAddChildModal(false)}
        onAdd={handleAddChild}
        title="Add Child Cell"
        excludeTypes={['hg']}
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
  icon: <Group />,
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
