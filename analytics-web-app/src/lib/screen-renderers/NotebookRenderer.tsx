import { useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { Plus, X, Trash2 } from 'lucide-react'
import {
  DndContext,
  closestCenter,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  DragEndEvent,
  DragStartEvent,
  DragOverEvent,
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
import type { CellConfig, VariableCellConfig, NotebookConfig, QueryCellConfig, HorizontalGroupCellConfig, VariableValue } from './notebook-types'
import { CellContainer } from '@/components/CellContainer'
import { CellEditor } from '@/components/CellEditor'
import { ResizeHandle } from '@/components/ResizeHandle'
import { Button } from '@/components/ui/button'
import { useNotebookVariables } from './useNotebookVariables'
import { useCellExecution, NotebookQueryEngine } from './useCellExecution'
import { cleanupVariableParams, resolveCellDataSource, flattenCellsForExecution, collectAllCellNames, validateCellName, sanitizeCellName } from './notebook-utils'
import { HorizontalGroupCell, HorizontalGroupCellEditor } from './cells/HorizontalGroupCell'
import { cleanupTimeParams, useExposeSaveRef } from '@/lib/url-cleanup-utils'
import { loadWasmEngine } from '@/lib/wasm-engine'
import { getTimeRangeForApi } from '@/lib/time-range'

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function formatElapsedMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`
  return `${(ms / 1000).toFixed(2)}s`
}

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
// HG Editor Panel
// ============================================================================

interface HgEditorPanelProps {
  config: HorizontalGroupCellConfig
  selectedChildName: string | null
  onChildSelect: (childName: string | null) => void
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  allCellNames: Set<string>
  defaultDataSource?: string
  onClose: () => void
  onUpdate: (updates: Partial<CellConfig>) => void
  onDelete: () => void
}

function HgEditorPanel({
  config,
  selectedChildName,
  onChildSelect,
  variables,
  timeRange,
  allCellNames,
  defaultDataSource,
  onClose,
  onUpdate,
  onDelete,
}: HgEditorPanelProps) {
  const [editedName, setEditedName] = useState(config.name)
  const [nameError, setNameError] = useState<string | null>(null)

  useEffect(() => {
    setEditedName(config.name)
    setNameError(null)
  }, [config.name])

  const handleNameChange = useCallback(
    (value: string) => {
      setEditedName(value)
      const error = validateCellName(value, allCellNames, config.name)
      if (error) {
        setNameError(error)
        return
      }
      setNameError(null)
      const sanitized = sanitizeCellName(value)
      onUpdate({ name: sanitized })
    },
    [onUpdate, config.name, allCellNames]
  )

  return (
    <div className="flex flex-col flex-1 min-h-0">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
        <div className="flex items-center gap-2">
          <span className="text-[11px] px-1.5 py-0.5 rounded bg-app-card text-theme-text-secondary uppercase font-medium">
            Group
          </span>
          <span className="font-medium text-theme-text-primary truncate">{config.name}</span>
        </div>
        <button
          onClick={onClose}
          className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors"
          title="Close"
        >
          <X className="w-5 h-5" />
        </button>
      </div>
      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {/* Group Name */}
        <div>
          <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
            Group Name
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
        {/* Children management */}
        <HorizontalGroupCellEditor
          config={config}
          onChange={(newConfig) => onUpdate(newConfig)}
          selectedChildName={selectedChildName}
          onChildSelect={onChildSelect}
          variables={variables}
          timeRange={timeRange}
          allCellNames={allCellNames}
          defaultDataSource={defaultDataSource}
        />
      </div>
      {/* Footer */}
      <div className="p-3 border-t border-theme-border space-y-2">
        <Button
          variant="outline"
          onClick={onDelete}
          className="w-full gap-2 text-accent-error border-accent-error hover:bg-accent-error/10"
        >
          <Trash2 className="w-4 h-4" />
          Delete Group
        </Button>
      </div>
    </div>
  )
}

// ============================================================================
// Main Component
// ============================================================================

export function NotebookRenderer({
  config,
  onConfigChange,
  savedConfig,
  rawTimeRange,
  onTimeRangeChange,
  onSave,
  refreshTrigger,
  onSaveRef,
  dataSource,
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
  useExposeSaveRef(onSaveRef, handleSave)

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

    // Update config with time range
    onConfigChange({
      ...notebookConfig,
      timeRangeFrom: current.from,
      timeRangeTo: current.to,
    })
  }, [rawTimeRange, savedNotebookConfig, notebookConfig, onConfigChange])

  // Variable values management - hook owns URL access for variables
  const { variableValues, variableValuesRef, setVariableValue, migrateVariable, removeVariable } =
    useNotebookVariables(
      cells,
      savedNotebookConfig?.cells ?? null,
    )

  // Auto-run: guard ref prevents re-entrance when auto-run itself sets variables
  const autoRunningRef = useRef(false)

  // Debounced auto-run timers (per cell name) for config changes like SQL editing
  const autoRunTimersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map())
  useEffect(() => {
    const timers = autoRunTimersRef.current
    return () => {
      for (const timer of timers.values()) clearTimeout(timer)
      timers.clear()
    }
  }, [])

  // WASM engine for notebook-local queries
  // Loaded eagerly so remote cell results are always registered for cross-cell references
  const [engine, setEngine] = useState<NotebookQueryEngine | null>(null)
  const [engineError, setEngineError] = useState<string | null>(null)

  useEffect(() => {
    if (engine) return
    let cancelled = false
    loadWasmEngine()
      .then((mod) => {
        if (!cancelled) {
          setEngine(new mod.WasmQueryEngine() as unknown as NotebookQueryEngine)
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setEngineError(err instanceof Error ? err.message : 'Failed to load WASM engine')
        }
      })
    return () => { cancelled = true }
  }, [engine])

  // Flatten hg children for execution (hg cells are replaced by their children)
  const executionCells = useMemo(() => flattenCellsForExecution(cells), [cells])

  // Cell execution state management (uses flattened list)
  const { cellStates, executeCell, executeFromCell, migrateCellState, removeCellState } = useCellExecution({
    cells: executionCells,
    rawTimeRange,
    variableValuesRef,
    setVariableValue,
    refreshTrigger,
    dataSource,
    engine,
  })

  // Execute a cell by name (finds it in the flat execution list)
  const executeCellByName = useCallback(
    (name: string) => {
      const idx = executionCells.findIndex((c) => c.name === name)
      if (idx !== -1) executeCell(idx)
    },
    [executionCells, executeCell]
  )

  // Execute from a cell by name (finds it in the flat execution list)
  const executeFromCellByName = useCallback(
    async (name: string) => {
      const idx = executionCells.findIndex((c) => c.name === name)
      if (idx !== -1) await executeFromCell(idx)
    },
    [executionCells, executeFromCell]
  )

  // Ref to always access the latest executeFromCellByName inside debounced timers
  const executeFromCellByNameRef = useRef(executeFromCellByName)
  executeFromCellByNameRef.current = executeFromCellByName

  // Schedule a debounced auto-run for a cell by name (used for SQL editing, content changes)
  const scheduleAutoRun = useCallback(
    (cellName: string) => {
      const timers = autoRunTimersRef.current
      const existing = timers.get(cellName)
      if (existing) clearTimeout(existing)
      timers.set(cellName, setTimeout(() => {
        timers.delete(cellName)
        executeFromCellByNameRef.current(cellName)
      }, 300))
    },
    [],
  )

  // Handle time range selection from charts (drag-to-zoom)
  const handleTimeRangeSelect = useCallback((from: Date, to: Date) => {
    onTimeRangeChange(from.toISOString(), to.toISOString())
  }, [onTimeRangeChange])

  // Re-execute table cells when sort options change (config is source of truth)
  // Scans both top-level cells and hg children
  const prevCellOptionsRef = useRef<Map<string, Record<string, unknown>>>(new Map())
  useEffect(() => {
    const checkCell = (cell: CellConfig) => {
      if (cell.type === 'table' || cell.type === 'log') {
        const cellConfig = cell as QueryCellConfig
        const prevOptions = prevCellOptionsRef.current.get(cell.name)
        const currentOptions = cellConfig.options

        const prevSortColumn = prevOptions?.sortColumn
        const prevSortDirection = prevOptions?.sortDirection
        const currentSortColumn = currentOptions?.sortColumn
        const currentSortDirection = currentOptions?.sortDirection

        if (
          prevOptions !== undefined &&
          (prevSortColumn !== currentSortColumn || prevSortDirection !== currentSortDirection)
        ) {
          executeCellByName(cell.name)
        }

        prevCellOptionsRef.current.set(cell.name, currentOptions ?? {})
      }
    }

    cells.forEach((cell) => {
      if (cell.type === 'hg') {
        (cell as HorizontalGroupCellConfig).children.forEach(checkCell)
      } else {
        checkCell(cell)
      }
    })
  }, [cells, executeCellByName])

  // UI state
  const [selectedCellIndex, setSelectedCellIndex] = useState<number | null>(null)
  const [selectedChildName, setSelectedChildName] = useState<string | null>(null)
  const [showAddCellModal, setShowAddCellModal] = useState(false)
  const [deletingCellIndex, setDeletingCellIndex] = useState<number | null>(null)
  const [activeDragId, setActiveDragId] = useState<string | null>(null)
  // Drag zone state for visual feedback (state) + synchronous read in handleDragEnd (refs)
  const [dragOverZone, setDragOverZone] = useState<'before' | 'into' | 'after' | null>(null)
  const [dragOverHgName, setDragOverHgName] = useState<string | null>(null)
  const dragOverZoneRef = useRef<'before' | 'into' | 'after' | null>(null)
  const dragOverHgNameRef = useRef<string | null>(null)
  const [showSource, setShowSource] = useState(false)

  useEffect(() => {
    if (!showSource) return
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setShowSource(false)
    }
    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [showSource])

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

  // Existing cell names for uniqueness check (includes hg children)
  const existingNames = useMemo(() => collectAllCellNames(cells), [cells])

  // Drag and drop
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates })
  )

  // Custom sorting strategy: suppress item transforms when dragging into the
  // "into" zone of an hg cell, so items don't shift as if reordering.
  const hgAwareSortingStrategy = useMemo<typeof verticalListSortingStrategy>(
    () => (args) => {
      if (dragOverZoneRef.current === 'into') {
        return null
      }
      return verticalListSortingStrategy(args)
    },
    []
  )

  const handleDragStart = useCallback((event: DragStartEvent) => {
    setActiveDragId(event.active.id as string)
  }, [])

  // Compute hg drop zone from a drag event's pointer position relative to the over element.
  // Returns the zone ('before'/'into'/'after') and hg name, or null if not over an hg cell.
  const computeHgZone = useCallback(
    (event: { activatorEvent: Event; delta: { x: number; y: number }; active: { id: string | number }; over: { id: string | number; rect: { top: number; height: number } } | null }) => {
      const { active, over } = event
      if (!over) return null

      const overCell = cells.find((c) => c.name === over.id)
      if (!overCell || overCell.type !== 'hg') return null

      // Don't allow dropping an hg cell into another hg
      const activeCell = cells.find((c) => c.name === active.id)
      if (activeCell?.type === 'hg') return null

      const overRect = over.rect
      if (!overRect || !overRect.height) return null

      const pointerY = (event.activatorEvent as PointerEvent)?.clientY
      if (pointerY === undefined) return null

      const currentY = pointerY + event.delta.y
      const relativeY = currentY - overRect.top
      const height = overRect.height

      // Wide "into" zone (80% of height) — the sortable list shifts items during
      // drag, so the pointer naturally lands near edges. Before/after reordering
      // is still possible by targeting the cells adjacent to the hg group.
      let zone: 'before' | 'into' | 'after'
      if (relativeY < height * 0.1) {
        zone = 'before'
      } else if (relativeY > height * 0.9) {
        zone = 'after'
      } else {
        zone = 'into'
      }

      return { zone, hgName: over.id as string }
    },
    [cells]
  )

  const handleDragOver = useCallback(
    (event: DragOverEvent) => {
      const result = computeHgZone(event)
      dragOverZoneRef.current = result?.zone ?? null
      dragOverHgNameRef.current = result?.hgName ?? null
      setDragOverZone(result?.zone ?? null)
      setDragOverHgName(result?.hgName ?? null)
    },
    [computeHgZone]
  )

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      // Compute zone directly from event data (no state/ref timing issues)
      const result = computeHgZone(event)
      setActiveDragId(null)
      setDragOverZone(null)
      setDragOverHgName(null)
      dragOverZoneRef.current = null
      dragOverHgNameRef.current = null

      const { active, over } = event
      if (!over || active.id === over.id) return

      const oldIndex = cells.findIndex((c) => c.name === active.id)
      const overIndex = cells.findIndex((c) => c.name === over.id)
      if (oldIndex === -1 || overIndex === -1) return

      // Check if dropping into an hg cell's middle zone
      const overCell = cells[overIndex]
      const activeCell = cells[oldIndex]
      if (
        overCell.type === 'hg' &&
        result?.zone === 'into' &&
        result?.hgName === over.id &&
        activeCell.type !== 'hg'
      ) {
        // Remove dragged cell from top-level and append to hg's children
        const hgCell = overCell as HorizontalGroupCellConfig
        const newCells = cells.filter((_, i) => i !== oldIndex)
        const hgIndex = newCells.findIndex((c) => c.name === hgCell.name)
        if (hgIndex !== -1) {
          newCells[hgIndex] = {
            ...hgCell,
            children: [...hgCell.children, activeCell],
          }
        }
        onConfigChange({ ...notebookConfig, cells: newCells })
        // Clear selection if the moved cell was selected
        if (selectedCellIndex === oldIndex) {
          setSelectedCellIndex(null)
          setSelectedChildName(null)
        } else if (selectedCellIndex !== null && selectedCellIndex > oldIndex) {
          setSelectedCellIndex(selectedCellIndex - 1)
        }
        return
      }

      // Standard reorder
      const newCells = arrayMove(cells, oldIndex, overIndex)
      const newConfig = { ...notebookConfig, cells: newCells }
      onConfigChange(newConfig)

      // Update selected cell index if needed
      if (selectedCellIndex === oldIndex) {
        setSelectedCellIndex(overIndex)
      } else if (selectedCellIndex !== null) {
        if (oldIndex < selectedCellIndex && overIndex >= selectedCellIndex) {
          setSelectedCellIndex(selectedCellIndex - 1)
        } else if (oldIndex > selectedCellIndex && overIndex <= selectedCellIndex) {
          setSelectedCellIndex(selectedCellIndex + 1)
        }
      }
    },
    [cells, notebookConfig, onConfigChange, selectedCellIndex, computeHgZone]
  )

  // Cell management
  const handleAddCell = useCallback(
    (type: CellType) => {
      const newCell = createDefaultCell(type, existingNames)
      const newCells = [...cells, newCell]
      const newConfig = { ...notebookConfig, cells: newCells }
      onConfigChange(newConfig)
      setShowAddCellModal(false)
      setSelectedCellIndex(newCells.length - 1)
    },
    [notebookConfig, cells, existingNames, onConfigChange]
  )

  const handleDeleteCell = useCallback(
    (index: number) => {
      const cell = cells[index]
      const newCells = cells.filter((_, i) => i !== index)
      const newConfig = { ...notebookConfig, cells: newCells }
      onConfigChange(newConfig)

      // Clean up state for the cell (and all children if hg)
      const cleanupCell = (c: CellConfig) => {
        removeCellState(c.name)
        if (c.type === 'variable') {
          removeVariable(c.name)
        }
        if (engine) {
          try { engine.deregister_table(c.name) } catch { /* ignore */ }
        }
      }

      cleanupCell(cell)
      if (cell.type === 'hg') {
        (cell as HorizontalGroupCellConfig).children.forEach(cleanupCell)
      }

      // Update selection
      if (selectedCellIndex === index) {
        setSelectedCellIndex(null)
        setSelectedChildName(null)
      } else if (selectedCellIndex !== null && selectedCellIndex > index) {
        setSelectedCellIndex(selectedCellIndex - 1)
      }
      setDeletingCellIndex(null)
    },
    [notebookConfig, cells, onConfigChange, selectedCellIndex, removeCellState, removeVariable, engine]
  )

  const handleDuplicateCell = useCallback(
    (index: number) => {
      const cell = cells[index]
      if (!cell) return

      const clone = structuredClone(cell)

      // Track all names (existing + those we generate) to avoid collisions
      const usedNames = new Set(existingNames)

      const uniquifyName = (baseName: string): string => {
        const copyBase = baseName + '_copy'
        let name = copyBase
        let counter = 1
        while (usedNames.has(name)) {
          counter++
          name = `${copyBase}_${counter}`
        }
        usedNames.add(name)
        return name
      }

      clone.name = uniquifyName(cell.name)

      // Uniquify children names for hg cells
      if (clone.type === 'hg') {
        const hgClone = clone as HorizontalGroupCellConfig
        hgClone.children = hgClone.children.map((child) => ({
          ...child,
          name: uniquifyName(child.name),
        }))
      }

      // Insert after the source cell
      const newCells = [...cells.slice(0, index + 1), clone, ...cells.slice(index + 1)]
      const newConfig = { ...notebookConfig, cells: newCells }
      onConfigChange(newConfig)
      setSelectedCellIndex(index + 1)
    },
    [cells, existingNames, notebookConfig, onConfigChange]
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

      // Auto-run: schedule debounced execution when a config value actually changed
      // (skip view-only keys that aren't execution-relevant)
      if (cell.autoRunFromHere) {
        const prev = cell as unknown as Record<string, unknown>
        const next = updates as unknown as Record<string, unknown>
        // Keys that affect presentation only, not query results
        const nonExecKeys = new Set(['layout', 'name', 'autoRunFromHere', 'options'])
        const hasChange = Object.keys(next).some(k => !nonExecKeys.has(k) && next[k] !== prev[k])
        if (hasChange) {
          scheduleAutoRun(cell.name)
        }
      }
    },
    [cells, notebookConfig, onConfigChange, migrateCellState, migrateVariable, setVariableValue, scheduleAutoRun]
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

  // Collect available variables for a cell at a given top-level index
  const getAvailableVariables = (index: number): Record<string, VariableValue> => {
    const available: Record<string, VariableValue> = {}
    for (let i = 0; i < index; i++) {
      const prevCell = cells[i]
      if (prevCell.type === 'variable' && variableValues[prevCell.name] !== undefined) {
        available[prevCell.name] = variableValues[prevCell.name]
      }
      // Also collect variable cells inside hg groups above
      if (prevCell.type === 'hg') {
        for (const child of (prevCell as HorizontalGroupCellConfig).children) {
          if (child.type === 'variable' && variableValues[child.name] !== undefined) {
            available[child.name] = variableValues[child.name]
          }
        }
      }
    }
    return available
  }

  const renderCell = (cell: CellConfig, index: number) => {
    const availableVariables = getAvailableVariables(index)

    // HG cell: render children side by side
    if (cell.type === 'hg') {
      const hgConfig = cell as HorizontalGroupCellConfig
      // For hg run button: run from first child
      const firstChildName = hgConfig.children[0]?.name
      const isDropTarget = activeDragId && dragOverHgName === cell.name
      const dropZoneClass = isDropTarget
        ? dragOverZone === 'into'
          ? 'ring-2 ring-accent-link ring-inset'
          : dragOverZone === 'before'
            ? 'border-t-4 border-t-[var(--accent-link)]'
            : dragOverZone === 'after'
              ? 'border-b-4 border-b-[var(--accent-link)]'
              : ''
        : ''
      return (
        <SortableCell key={cell.name} id={cell.name}>
          {({ dragHandleProps, isDragging, setNodeRef, style }) => (
            <div ref={setNodeRef} style={style} className={dropZoneClass}>
              <CellContainer
                dragHandleProps={dragHandleProps}
                isDragging={isDragging}
                name={cell.name}
                type={cell.type}
                status="idle"
                collapsed={cell.layout.collapsed}
                onToggleCollapsed={() => toggleCellCollapsed(index)}
                isSelected={selectedCellIndex === index}
                onSelect={() => {
                  setSelectedCellIndex(index)
                  setSelectedChildName(null)
                }}
                onRun={firstChildName ? () => executeCellByName(firstChildName) : undefined}
                onRunFromHere={firstChildName ? () => executeFromCellByName(firstChildName) : undefined}
                onDuplicate={() => handleDuplicateCell(index)}
                onDelete={() => setDeletingCellIndex(index)}
                height={cell.layout.height}
                onHeightChange={(newHeight) =>
                  updateCell(index, { layout: { ...cell.layout, height: newHeight } })
                }
              >
                <HorizontalGroupCell
                config={hgConfig}
                cellStates={cellStates}
                variables={availableVariables}
                variableValues={variableValues}
                timeRange={getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to)}
                selectedChildName={selectedChildName}
                onChildSelect={(childName) => {
                  setSelectedCellIndex(index)
                  setSelectedChildName(childName)
                }}
                onChildRun={(childName) => executeCellByName(childName)}
                onVariableValueChange={(cellName, value) => {
                  setVariableValue(cellName, value)
                }}
                onConfigChange={(newHgConfig) => {
                  updateCell(index, newHgConfig)
                }}
                onChildDragOut={(childName, position) => {
                  const child = hgConfig.children.find((c) => c.name === childName)
                  if (!child) return
                  const newChildren = hgConfig.children.filter((c) => c.name !== childName)
                  const newCells = [...cells]
                  newCells[index] = { ...hgConfig, children: newChildren }
                  // Insert before or after the hg group based on drag direction
                  const insertIndex = position === 'before' ? index : index + 1
                  newCells.splice(insertIndex, 0, child)
                  onConfigChange({ ...notebookConfig, cells: newCells })
                  if (selectedChildName === childName) {
                    setSelectedChildName(null)
                  }
                }}
                allCellNames={existingNames}
              />
              </CellContainer>
            </div>
          )}
        </SortableCell>
      )
    }

    // Regular cell rendering
    const state = cellStates[cell.name] || { status: 'idle', data: [] }
    const meta = getCellTypeMetadata(cell.type)
    const CellRenderer = getCellRenderer(cell.type)
    const rendererProps = meta.getRendererProps(cell, state)

    // Determine status text
    const isNonComboVariable = cell.type === 'variable' && (cell as VariableCellConfig).variableType !== 'combobox'
    let statusText: string | undefined
    if (isNonComboVariable) {
      statusText = undefined
    } else if (state.status === 'loading' && state.fetchProgress) {
      statusText = `${state.fetchProgress.rows.toLocaleString()} rows (${formatBytes(state.fetchProgress.bytes)})`
    } else if (state.data.length > 0) {
      const totalRows = state.data.reduce((sum, t) => sum + t.numRows, 0)
      const totalBytes = state.data.reduce(
        (sum, t) => sum + t.batches.reduce((s: number, b) => s + b.data.byteLength, 0), 0
      )
      const rowText = `${totalRows.toLocaleString()} rows (${formatBytes(totalBytes)})`
      statusText = state.elapsedMs != null ? `${rowText} in ${formatElapsedMs(state.elapsedMs)}` : rowText
    }

    // Effective data source: per-cell overrides notebook-level, resolve $varname references
    const cellDataSource = resolveCellDataSource(cell, availableVariables, dataSource)

    // Build common renderer props
    const commonRendererProps = {
      name: cell.name,
      data: state.data,
      status: state.status,
      error: state.error,
      timeRange: getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to),
      variables: availableVariables,
      isEditing: selectedCellIndex === index,
      onRun: () => executeCellByName(cell.name),
      onSqlChange: (sql: string) => updateCell(index, { sql }),
      onOptionsChange: (options: Record<string, unknown>) => updateCell(index, { options }),
      onContentChange: (content: string) => updateCell(index, { content }),
      onTimeRangeSelect: handleTimeRangeSelect,
      value: cell.type === 'variable' ? variableValues[cell.name] : undefined,
      onValueChange: cell.type === 'variable' ? (value: VariableValue) => {
        setVariableValue(cell.name, value)

        // Auto-run: if this cell has autoRunFromHere, execute from here onward.
        if (autoRunningRef.current || !cell.autoRunFromHere) return
        autoRunningRef.current = true
        executeFromCellByName(cell.name).then(() => {
          autoRunningRef.current = false
        })
      } : undefined,
      dataSource: cellDataSource,
      ...rendererProps,
    }

    // Render title bar content if metadata defines a titleBarRenderer
    const TitleBarRenderer = meta.titleBarRenderer
    const titleBarContent = TitleBarRenderer ? <TitleBarRenderer {...commonRendererProps} /> : undefined
    const autoCollapse = !!TitleBarRenderer
    const collapsed = autoCollapse ? state.status !== 'error' : cell.layout.collapsed
    const onToggleCollapsed = autoCollapse ? undefined : () => toggleCellCollapsed(index)

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
            collapsed={collapsed}
            onToggleCollapsed={onToggleCollapsed}
            isSelected={selectedCellIndex === index}
            onSelect={() => {
              setSelectedCellIndex(index)
              setSelectedChildName(null)
            }}
            onRun={() => executeCellByName(cell.name)}
            onRunFromHere={() => executeFromCellByName(cell.name)}
            autoRunFromHere={cell.autoRunFromHere}
            onToggleAutoRunFromHere={() =>
              updateCell(index, { autoRunFromHere: !cell.autoRunFromHere })
            }
            onDuplicate={() => handleDuplicateCell(index)}
            onDelete={() => setDeletingCellIndex(index)}
            statusText={statusText}
            height={cell.layout.height}
            onHeightChange={(newHeight) =>
              updateCell(index, { layout: { ...cell.layout, height: newHeight } })
            }
            titleBarContent={titleBarContent}
          >
            <CellRenderer {...commonRendererProps} />
          </CellContainer>
        )}
      </SortableCell>
    )
  }

  return (
    <div className="flex h-full">
      {/* Main content area */}
      <div className="flex-1 flex flex-col p-6 min-w-0 overflow-auto">
        {engineError && (
          <div className="mb-3 px-4 py-2 bg-accent-error/10 border border-accent-error/30 rounded text-xs text-accent-error">
            WASM engine failed to load: {engineError}
          </div>
        )}
        {showSource ? (
          <div className="flex flex-col gap-4">
            <div className="flex items-center gap-3">
              <button
                onClick={() => setShowSource(false)}
                className="text-sm text-accent-link hover:underline"
              >
                &larr; Back to notebook
              </button>
              <span className="text-[11px] px-1.5 py-0.5 rounded bg-app-card text-theme-text-secondary font-mono font-medium">
                JSON
              </span>
              <span className="text-sm text-theme-text-primary font-medium">Notebook Configuration</span>
              <span className="text-xs text-theme-text-muted">read-only</span>
            </div>
            <pre className="bg-app-card border border-theme-border rounded-lg p-4 overflow-auto text-xs font-mono text-theme-text-secondary whitespace-pre">
              {JSON.stringify(notebookConfig, null, 2)}
            </pre>
          </div>
        ) : (
          <>
            <DndContext
              sensors={sensors}
              collisionDetection={closestCenter}
              onDragStart={handleDragStart}
              onDragOver={handleDragOver}
              onDragEnd={handleDragEnd}
            >
              <SortableContext items={cells.map((c) => c.name)} strategy={hgAwareSortingStrategy}>
                <div className="flex flex-col gap-3">
                  {cells.map((cell, index) => renderCell(cell, index))}

                  <button
                    onClick={() => setShowAddCellModal(true)}
                    className="w-full py-3 border-2 border-dashed border-theme-border rounded-lg bg-transparent text-theme-text-muted hover:border-accent-link hover:text-accent-link hover:bg-accent-link/10 transition-colors"
                  >
                    <Plus className="w-4 h-4 inline-block mr-2" />
                    Add Cell
                  </button>

                  <button
                    onClick={() => setShowSource(true)}
                    className="text-xs text-theme-text-muted hover:text-theme-text-secondary transition-colors py-2 self-center"
                  >
                    {'{ }'} View source
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
          </>
        )}
      </div>

      {/* Right panel - Cell Editor */}
      {!showSource && selectedCell && (
        <>
          <ResizeHandle orientation="horizontal" onResize={handleEditorPanelResize} />
          <div
            className="h-full bg-app-panel border-l border-theme-border flex flex-col flex-shrink-0 overflow-hidden"
            style={{ width: editorPanelWidth }}
          >
            {selectedCell.type === 'hg' ? (
              <HgEditorPanel
                config={selectedCell as HorizontalGroupCellConfig}
                selectedChildName={selectedChildName}
                onChildSelect={setSelectedChildName}
                variables={variableValues}
                timeRange={getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to)}
                allCellNames={existingNames}
                defaultDataSource={dataSource}
                onClose={() => { setSelectedCellIndex(null); setSelectedChildName(null) }}
                onUpdate={(updates) => updateCell(selectedCellIndex!, updates)}
                onDelete={() => setDeletingCellIndex(selectedCellIndex!)}
              />
            ) : (
              <CellEditor
                cell={selectedCell}
                variables={variableValues}
                timeRange={getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to)}
                existingNames={existingNames}
                availableColumns={cellStates[selectedCell.name]?.data[0]?.schema.fields.map((f) => f.name)}
                defaultDataSource={dataSource}
                showNotebookOption
                datasourceVariables={
                  selectedCellIndex !== null
                    ? cells
                        .slice(0, selectedCellIndex)
                        .filter((c) =>
                          c.type === 'variable' && (c as VariableCellConfig).variableType === 'datasource'
                        )
                        .map((c) => c.name)
                    : undefined
                }
                onClose={() => { setSelectedCellIndex(null); setSelectedChildName(null) }}
                onUpdate={(updates) => updateCell(selectedCellIndex!, updates)}
                onRun={() => executeCellByName(selectedCell.name)}
                onDelete={() => setDeletingCellIndex(selectedCellIndex!)}
              />
            )}
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
