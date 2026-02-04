import { useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { Plus, X } from 'lucide-react'
import {
  DndContext,
  closestCenter,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  DragEndEvent,
  DragStartEvent,
  DragOverlay,
} from '@dnd-kit/core'
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable'
import { CSS } from '@dnd-kit/utilities'
import { registerRenderer, ScreenRendererProps } from './index'
import {
  CellType,
  getCellRenderer,
  getCellTypeMetadata,
  CELL_TYPE_OPTIONS,
  createDefaultCell,
} from './cell-registry'
import type { CellConfig, VariableCellConfig, NotebookConfig, QueryCellConfig, VariableValue } from './notebook-types'
import { CellContainer } from '@/components/CellContainer'
import { CellEditor } from '@/components/CellEditor'
import { ResizeHandle } from '@/components/ResizeHandle'
import { Button } from '@/components/ui/button'
import { SaveFooter } from './shared'
import { useNotebookVariables } from './useNotebookVariables'
import { useCellExecution } from './useCellExecution'
import { notebookConfigsEqual, cleanupVariableParams } from './notebook-utils'
import { cleanupTimeParams } from '@/lib/url-cleanup-utils'
import { DEFAULT_TIME_RANGE } from '@/lib/screen-defaults'

// ============================================================================
// Constants
// ============================================================================

const EDITOR_PANEL_MIN_WIDTH = 280
const EDITOR_PANEL_MAX_WIDTH = 800
const EDITOR_PANEL_DEFAULT_WIDTH = 350

// ============================================================================
// Modal Components
// ============================================================================

interface AddCellModalProps {
  isOpen: boolean
  onClose: () => void
  onAdd: (type: CellType) => void
}

function AddCellModal({ isOpen, onClose, onAdd }: AddCellModalProps) {
  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={onClose} />
      <div className="relative w-full max-w-sm bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
          <h2 className="text-lg font-medium text-theme-text-primary">Add Cell</h2>
          <button
            onClick={onClose}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>
        <div className="p-2">
          {CELL_TYPE_OPTIONS.map((option) => (
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

interface DeleteCellModalProps {
  isOpen: boolean
  cellName: string
  onClose: () => void
  onConfirm: () => void
}

function DeleteCellModal({ isOpen, cellName, onClose, onConfirm }: DeleteCellModalProps) {
  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/50" onClick={onClose} />
      <div className="relative w-full max-w-sm bg-app-panel border border-theme-border rounded-lg shadow-xl">
        <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
          <h2 className="text-lg font-medium text-theme-text-primary">Delete Cell?</h2>
          <button
            onClick={onClose}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>
        <div className="p-4">
          <p className="text-sm text-theme-text-secondary">
            Are you sure you want to delete &quot;{cellName}&quot;? This action cannot be undone.
          </p>
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-theme-border">
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={onConfirm} className="bg-[var(--accent-error)] hover:bg-[var(--accent-error-bright)] text-white">
            Delete
          </Button>
        </div>
      </div>
    </div>
  )
}

// ============================================================================
// Sortable Cell Wrapper
// ============================================================================

interface SortableCellProps {
  id: string
  children: (props: {
    dragHandleProps: Record<string, unknown>
    isDragging: boolean
    setNodeRef: (node: HTMLElement | null) => void
    style: React.CSSProperties
  }) => React.ReactNode
}

function SortableCell({ id, children }: SortableCellProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id })

  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
  }

  return <>{children({ dragHandleProps: { ...attributes, ...listeners }, isDragging, setNodeRef, style })}</>
}

// ============================================================================
// Main Component
// ============================================================================

export function NotebookRenderer({
  config,
  onConfigChange,
  savedConfig,
  setHasUnsavedChanges,
  timeRange,
  rawTimeRange,
  onTimeRangeChange,
  onSave,
  isSaving,
  hasUnsavedChanges,
  onSaveAs,
  saveError,
  refreshTrigger,
}: ScreenRendererProps) {
  const [, setSearchParams] = useSearchParams()

  // Wrap onSave: call parent save, then cleanup both time and variable params in one navigation
  const handleSave = useMemo(() => {
    if (!onSave) return null
    return async () => {
      const savedConfig = await onSave()
      if (savedConfig) {
        setSearchParams(prev => {
          const next = new URLSearchParams(prev)
          cleanupTimeParams(next, savedConfig)
          cleanupVariableParams(next, savedConfig)
          return next
        })
      }
    }
  }, [onSave, setSearchParams])

  // Parse config
  const notebookConfig = useMemo(() => {
    const cfg = config as unknown as NotebookConfig | null
    return cfg && cfg.cells ? cfg : { cells: [] }
  }, [config])

  // Parse saved config for comparison
  const savedNotebookConfig = useMemo(() => {
    const cfg = savedConfig as unknown as NotebookConfig | null
    return cfg && cfg.cells ? cfg : null
  }, [savedConfig])

  const cells = notebookConfig.cells

  // Helper to update unsaved state based on config comparison
  const updateUnsavedState = useCallback(
    (newConfig: NotebookConfig) => {
      setHasUnsavedChanges(!notebookConfigsEqual(newConfig, savedNotebookConfig))
    },
    [savedNotebookConfig, setHasUnsavedChanges]
  )

  // Sync time range changes to config
  // When time range changes, update config and check unsaved state
  const prevTimeRangeRef = useRef<{ from: string; to: string } | null>(null)
  useEffect(() => {
    const current = { from: rawTimeRange.from, to: rawTimeRange.to }

    // On first run, just store current values
    if (prevTimeRangeRef.current === null) {
      prevTimeRangeRef.current = current
      return
    }

    const prev = prevTimeRangeRef.current
    if (prev.from === current.from && prev.to === current.to) {
      return
    }

    prevTimeRangeRef.current = current

    // Check if time range differs from saved config
    const savedFrom = savedNotebookConfig?.timeRangeFrom ?? DEFAULT_TIME_RANGE.from
    const savedTo = savedNotebookConfig?.timeRangeTo ?? DEFAULT_TIME_RANGE.to
    const timeRangeDiffers = current.from !== savedFrom || current.to !== savedTo

    // Create config with updated time range for unsaved state check
    const configWithTimeRange: NotebookConfig = {
      ...notebookConfig,
      timeRangeFrom: current.from,
      timeRangeTo: current.to,
    }
    const hasCellChanges = !notebookConfigsEqual(configWithTimeRange, savedNotebookConfig)
    setHasUnsavedChanges(hasCellChanges || timeRangeDiffers)

    // Update config with time range
    onConfigChange({
      ...notebookConfig,
      timeRangeFrom: current.from,
      timeRangeTo: current.to,
    })
  }, [rawTimeRange, savedNotebookConfig, notebookConfig, setHasUnsavedChanges, onConfigChange])

  // Variable values management - hook owns URL access for variables
  const { variableValues, variableValuesRef, setVariableValue, migrateVariable, removeVariable } =
    useNotebookVariables(
      cells,
      savedNotebookConfig?.cells ?? null,
    )

  // Cell execution state management
  const { cellStates, executeCell, executeFromCell, migrateCellState, removeCellState } = useCellExecution({
    cells,
    timeRange,
    variableValuesRef,
    setVariableValue,
    refreshTrigger,
  })

  // Handle time range selection from charts (drag-to-zoom)
  const handleTimeRangeSelect = useCallback((from: Date, to: Date) => {
    onTimeRangeChange(from.toISOString(), to.toISOString())
  }, [onTimeRangeChange])

  // Re-execute table cells when sort options change (config is source of truth)
  const prevCellOptionsRef = useRef<Map<string, Record<string, unknown>>>(new Map())
  useEffect(() => {
    cells.forEach((cell, index) => {
      if (cell.type === 'table' || cell.type === 'log') {
        const cellConfig = cell as QueryCellConfig
        const prevOptions = prevCellOptionsRef.current.get(cell.name)
        const currentOptions = cellConfig.options

        // Check if sort state changed
        const prevSortColumn = prevOptions?.sortColumn
        const prevSortDirection = prevOptions?.sortDirection
        const currentSortColumn = currentOptions?.sortColumn
        const currentSortDirection = currentOptions?.sortDirection

        if (
          prevOptions !== undefined &&
          (prevSortColumn !== currentSortColumn || prevSortDirection !== currentSortDirection)
        ) {
          executeCell(index)
        }

        // Update tracked options
        prevCellOptionsRef.current.set(cell.name, currentOptions ?? {})
      }
    })
  }, [cells, executeCell])

  // UI state
  const [selectedCellIndex, setSelectedCellIndex] = useState<number | null>(null)
  const [showAddCellModal, setShowAddCellModal] = useState(false)
  const [deletingCellIndex, setDeletingCellIndex] = useState<number | null>(null)
  const [activeDragId, setActiveDragId] = useState<string | null>(null)

  // Editor panel width (persisted to localStorage)
  const [editorPanelWidth, setEditorPanelWidth] = useState(() => {
    const saved = localStorage.getItem('notebook-editor-panel-width')
    return saved ? parseInt(saved, 10) : EDITOR_PANEL_DEFAULT_WIDTH
  })

  useEffect(() => {
    localStorage.setItem('notebook-editor-panel-width', String(editorPanelWidth))
  }, [editorPanelWidth])

  const handleEditorPanelResize = useCallback((delta: number) => {
    setEditorPanelWidth((prev) =>
      Math.max(EDITOR_PANEL_MIN_WIDTH, Math.min(EDITOR_PANEL_MAX_WIDTH, prev - delta))
    )
  }, [])

  // Existing cell names for uniqueness check
  const existingNames = useMemo(() => new Set(cells.map((c) => c.name)), [cells])

  // Drag and drop
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates })
  )

  const handleDragStart = useCallback((event: DragStartEvent) => {
    setActiveDragId(event.active.id as string)
  }, [])

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      setActiveDragId(null)
      const { active, over } = event
      if (!over || active.id === over.id) return

      const oldIndex = cells.findIndex((c) => c.name === active.id)
      const newIndex = cells.findIndex((c) => c.name === over.id)
      if (oldIndex === -1 || newIndex === -1) return

      const newCells = arrayMove(cells, oldIndex, newIndex)
      const newConfig = { ...notebookConfig, cells: newCells }
      onConfigChange(newConfig)
      updateUnsavedState(newConfig)

      // Update selected cell index if needed
      if (selectedCellIndex === oldIndex) {
        setSelectedCellIndex(newIndex)
      } else if (selectedCellIndex !== null) {
        if (oldIndex < selectedCellIndex && newIndex >= selectedCellIndex) {
          setSelectedCellIndex(selectedCellIndex - 1)
        } else if (oldIndex > selectedCellIndex && newIndex <= selectedCellIndex) {
          setSelectedCellIndex(selectedCellIndex + 1)
        }
      }
    },
    [cells, notebookConfig, onConfigChange, updateUnsavedState, selectedCellIndex]
  )

  // Cell management
  const handleAddCell = useCallback(
    (type: CellType) => {
      const newCell = createDefaultCell(type, existingNames)
      const newCells = [...cells, newCell]
      const newConfig = { ...notebookConfig, cells: newCells }
      onConfigChange(newConfig)
      updateUnsavedState(newConfig)
      setShowAddCellModal(false)
      setSelectedCellIndex(newCells.length - 1)
    },
    [notebookConfig, cells, existingNames, onConfigChange, updateUnsavedState]
  )

  const handleDeleteCell = useCallback(
    (index: number) => {
      const cell = cells[index]
      const newCells = cells.filter((_, i) => i !== index)
      const newConfig = { ...notebookConfig, cells: newCells }
      onConfigChange(newConfig)
      updateUnsavedState(newConfig)

      // Clean up state
      removeCellState(cell.name)
      if (cell.type === 'variable') {
        removeVariable(cell.name)
      }

      // Update selection
      if (selectedCellIndex === index) {
        setSelectedCellIndex(null)
      } else if (selectedCellIndex !== null && selectedCellIndex > index) {
        setSelectedCellIndex(selectedCellIndex - 1)
      }
      setDeletingCellIndex(null)
    },
    [notebookConfig, cells, onConfigChange, updateUnsavedState, selectedCellIndex, removeCellState, removeVariable]
  )

  const updateCell = useCallback(
    (index: number, updates: Partial<CellConfig>) => {
      const cell = cells[index]
      if (!cell) return

      const newCells = [...cells]
      newCells[index] = { ...cell, ...updates } as CellConfig

      // Handle rename: migrate state to new name
      if (updates.name && updates.name !== cell.name) {
        migrateCellState(cell.name, updates.name)
        if (cell.type === 'variable') {
          migrateVariable(cell.name, updates.name)
        }
      }

      // Handle defaultValue change for variable cells:
      // When user edits the default value, update current value to match
      // This uses delta logic - if new default matches baseline, URL param is removed
      if (cell.type === 'variable' && 'defaultValue' in updates) {
        const newDefault = (updates as Partial<VariableCellConfig>).defaultValue
        if (newDefault !== undefined) {
          setVariableValue(cell.name, newDefault)
        }
      }

      const newConfig = { ...notebookConfig, cells: newCells }
      onConfigChange(newConfig)
      updateUnsavedState(newConfig)
    },
    [cells, notebookConfig, onConfigChange, updateUnsavedState, migrateCellState, migrateVariable, setVariableValue]
  )

  const toggleCellCollapsed = useCallback(
    (index: number) => {
      const cell = cells[index]
      updateCell(index, { layout: { ...cell.layout, collapsed: !cell.layout.collapsed } })
    },
    [cells, updateCell]
  )

  // Render
  const selectedCell = selectedCellIndex !== null ? cells[selectedCellIndex] : null

  const renderCell = (cell: CellConfig, index: number) => {
    const state = cellStates[cell.name] || { status: 'idle', data: null }
    const meta = getCellTypeMetadata(cell.type)
    const CellRenderer = getCellRenderer(cell.type)

    // Variables available to this cell (from cells above)
    const availableVariables: Record<string, VariableValue> = {}
    for (let i = 0; i < index; i++) {
      const prevCell = cells[i]
      if (prevCell.type === 'variable' && variableValues[prevCell.name] !== undefined) {
        availableVariables[prevCell.name] = variableValues[prevCell.name]
      }
    }

    // Get type-specific props from metadata
    const rendererProps = meta.getRendererProps(cell, state)

    // Determine status text
    const statusText =
      cell.type === 'variable' && (cell as VariableCellConfig).variableType !== 'combobox'
        ? undefined
        : state.data
          ? `${state.data.numRows} rows`
          : undefined

    return (
      <SortableCell key={cell.name} id={cell.name}>
        {({ dragHandleProps, isDragging, setNodeRef, style }) => (
          <CellContainer
            ref={setNodeRef}
            style={style}
            dragHandleProps={dragHandleProps}
            isDragging={isDragging}
            name={cell.name}
            type={cell.type}
            status={state.status}
            error={state.error}
            collapsed={cell.layout.collapsed}
            onToggleCollapsed={() => toggleCellCollapsed(index)}
            isSelected={selectedCellIndex === index}
            onSelect={() => setSelectedCellIndex(index)}
            onRun={() => executeCell(index)}
            onRunFromHere={() => executeFromCell(index)}
            onDelete={() => setDeletingCellIndex(index)}
            statusText={statusText}
            height={cell.layout.height}
            onHeightChange={(newHeight) =>
              updateCell(index, { layout: { ...cell.layout, height: newHeight } })
            }
          >
            <CellRenderer
              name={cell.name}
              data={state.data}
              status={state.status}
              error={state.error}
              timeRange={timeRange}
              variables={availableVariables}
              isEditing={selectedCellIndex === index}
              onRun={() => executeCell(index)}
              onSqlChange={(sql) => updateCell(index, { sql })}
              onOptionsChange={(options) => updateCell(index, { options })}
              onContentChange={(content) => updateCell(index, { content })}
              onTimeRangeSelect={handleTimeRangeSelect}
              value={cell.type === 'variable' ? variableValues[cell.name] : undefined}
              onValueChange={cell.type === 'variable' ? (value) => setVariableValue(cell.name, value) : undefined}
              {...rendererProps}
            />
          </CellContainer>
        )}
      </SortableCell>
    )
  }

  return (
    <div className="flex h-full">
      {/* Main content area */}
      <div className="flex-1 flex flex-col p-6 min-w-0 overflow-auto">
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragStart={handleDragStart}
          onDragEnd={handleDragEnd}
        >
          <SortableContext items={cells.map((c) => c.name)} strategy={verticalListSortingStrategy}>
            <div className="flex flex-col gap-3">
              {cells.map((cell, index) => renderCell(cell, index))}

              <button
                onClick={() => setShowAddCellModal(true)}
                className="w-full py-3 border-2 border-dashed border-theme-border rounded-lg bg-transparent text-theme-text-muted hover:border-accent-link hover:text-accent-link hover:bg-accent-link/10 transition-colors"
              >
                <Plus className="w-4 h-4 inline-block mr-2" />
                Add Cell
              </button>
            </div>
          </SortableContext>
          <DragOverlay>
            {activeDragId ? (
              <div className="bg-app-panel border-2 border-accent-link rounded-lg shadow-xl opacity-90">
                <div className="flex items-center gap-2 px-3 py-2 bg-app-card rounded-t-lg">
                  <span className="text-[11px] px-1.5 py-0.5 rounded bg-app-panel text-theme-text-secondary uppercase font-medium">
                    {cells.find((c) => c.name === activeDragId)?.type}
                  </span>
                  <span className="font-medium text-theme-text-primary">{activeDragId}</span>
                </div>
              </div>
            ) : null}
          </DragOverlay>
        </DndContext>
      </div>

      {/* Right panel - Cell Editor */}
      {selectedCell && (
        <>
          <ResizeHandle orientation="horizontal" onResize={handleEditorPanelResize} />
          <div
            className="h-full bg-app-panel border-l border-theme-border flex flex-col flex-shrink-0 overflow-hidden"
            style={{ width: editorPanelWidth }}
          >
            <CellEditor
              cell={selectedCell}
              variables={variableValues}
              timeRange={timeRange}
              existingNames={existingNames}
              availableColumns={cellStates[selectedCell.name]?.data?.schema.fields.map((f) => f.name)}
              onClose={() => setSelectedCellIndex(null)}
              onUpdate={(updates) => updateCell(selectedCellIndex!, updates)}
              onRun={() => executeCell(selectedCellIndex!)}
              onDelete={() => setDeletingCellIndex(selectedCellIndex!)}
            />
            <div className="border-t border-theme-border flex-shrink-0">
              <SaveFooter
                onSave={handleSave}
                onSaveAs={onSaveAs}
                isSaving={isSaving}
                hasUnsavedChanges={hasUnsavedChanges}
                saveError={saveError}
              />
            </div>
          </div>
        </>
      )}

      {/* Modals */}
      <AddCellModal isOpen={showAddCellModal} onClose={() => setShowAddCellModal(false)} onAdd={handleAddCell} />
      <DeleteCellModal
        isOpen={deletingCellIndex !== null}
        cellName={deletingCellIndex !== null ? cells[deletingCellIndex]?.name || '' : ''}
        onClose={() => setDeletingCellIndex(null)}
        onConfirm={() => deletingCellIndex !== null && handleDeleteCell(deletingCellIndex)}
      />
    </div>
  )
}

// Register this renderer
registerRenderer('notebook', NotebookRenderer)
