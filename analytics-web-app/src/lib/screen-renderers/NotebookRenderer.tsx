import { useState, useCallback, useEffect, useRef, useMemo } from 'react'
import { Plus, X } from 'lucide-react'
import { Table } from 'apache-arrow'
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
import { CellType, CellStatus, getCellRenderer } from './cell-registry'
import { CellContainer } from '@/components/CellContainer'
import { CellEditor } from '@/components/CellEditor'
import { streamQuery } from '@/lib/arrow-stream'
import { Button } from '@/components/ui/button'
import { SaveFooter } from './shared'

// ============================================================================
// Types (owned by NotebookRenderer, not shared)
// ============================================================================

/**
 * Config for screens with type: 'notebook'.
 * Time range is handled at the screen level, same as other screen types.
 */
interface NotebookConfig {
  cells: CellConfig[]
  refreshInterval?: number
  timeRangeFrom?: string
  timeRangeTo?: string
}

type CellConfig = QueryCellConfig | MarkdownCellConfig | VariableCellConfig

interface CellConfigBase {
  /** Unique within notebook; display name + anchor for deep linking */
  name: string
  /** Cell type - for variable cells, name is also the variable name ($name) */
  type: CellType
  /** Layout settings */
  layout: { height: number | 'auto'; collapsed?: boolean }
}

interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log'
  sql: string
  /** Cell-specific options (e.g., chart: { xColumn, yColumn }) */
  options?: Record<string, unknown>
}

interface MarkdownCellConfig extends CellConfigBase {
  type: 'markdown'
  content: string
}

interface VariableCellConfig extends CellConfigBase {
  type: 'variable'
  variableType: 'combobox' | 'text' | 'number'
  /** For combobox: query to populate options (1 col = value+label, 2 cols = value, label) */
  sql?: string
  defaultValue?: string
}

/** Execution state for a cell */
interface CellState {
  status: CellStatus
  error?: string
  data: Table | null
  /** For variable cells (combobox): options loaded from query */
  variableOptions?: { label: string; value: string }[]
}

// Default SQL queries per cell type
const DEFAULT_SQL: Record<string, string> = {
  table: `SELECT process_id, exe, start_time, last_update_time, username, computer
FROM processes
ORDER BY last_update_time DESC
LIMIT 100`,
  chart: `SELECT time, value
FROM measures
WHERE name = 'cpu_usage'
ORDER BY time
LIMIT 100`,
  log: `SELECT time, level, target, msg
FROM log_entries
ORDER BY time DESC
LIMIT 100`,
  variable: `SELECT DISTINCT name FROM measures`,
}

// Cell type options for the add cell modal
const CELL_TYPE_OPTIONS: { type: CellType; name: string; description: string; icon: string }[] = [
  { type: 'table', name: 'Table', description: 'Generic SQL results as a table', icon: 'T' },
  { type: 'chart', name: 'Chart', description: 'X/Y chart (line, bar, etc.)', icon: 'C' },
  { type: 'log', name: 'Log', description: 'Log entries viewer with levels', icon: 'L' },
  { type: 'markdown', name: 'Markdown', description: 'Documentation and notes', icon: 'M' },
  { type: 'variable', name: 'Variable', description: 'User input (dropdown, text, number)', icon: 'V' },
]

// ============================================================================
// Helper functions
// ============================================================================

function createDefaultCell(type: CellType, existingNames: Set<string>): CellConfig {
  // Generate unique name
  const baseName = type.charAt(0).toUpperCase() + type.slice(1)
  let name = baseName
  let counter = 1
  while (existingNames.has(name)) {
    counter++
    name = `${baseName} ${counter}`
  }

  const baseConfig: CellConfigBase = {
    name,
    type,
    layout: { height: 'auto' },
  }

  switch (type) {
    case 'table':
    case 'chart':
    case 'log':
      return { ...baseConfig, type, sql: DEFAULT_SQL[type] } as QueryCellConfig
    case 'markdown':
      return { ...baseConfig, type: 'markdown', content: '# Notes\n\nAdd your documentation here.' } as MarkdownCellConfig
    case 'variable':
      return {
        ...baseConfig,
        type: 'variable',
        variableType: 'combobox',
        sql: DEFAULT_SQL.variable,
      } as VariableCellConfig
    default:
      return { ...baseConfig, type: 'table', sql: DEFAULT_SQL.table } as QueryCellConfig
  }
}

function substituteMacros(sql: string, variables: Record<string, string>, timeRange: { begin: string; end: string }): string {
  let result = sql
  // Substitute $begin and $end (these are timestamps, keep quotes)
  result = result.replace(/\$begin/g, `'${timeRange.begin}'`)
  result = result.replace(/\$end/g, `'${timeRange.end}'`)
  // Substitute user variables - don't add quotes, let the SQL author control quoting
  // Sort by name length descending to avoid partial matches ($metric vs $metric_name)
  const sortedVars = Object.entries(variables).sort((a, b) => b[0].length - a[0].length)
  for (const [name, value] of sortedVars) {
    const regex = new RegExp(`\\$${name}\\b`, 'g')
    // Escape single quotes in value for SQL safety
    const escaped = value.replace(/'/g, "''")
    result = result.replace(regex, escaped)
  }
  return result
}

// Execute a single SQL query and return the result table
async function executeSql(
  sql: string,
  timeRange: { begin: string; end: string },
  abortSignal: AbortSignal
): Promise<Table> {
  const batches: import('apache-arrow').RecordBatch[] = []

  for await (const result of streamQuery(
    {
      sql,
      params: { begin: timeRange.begin, end: timeRange.end },
      begin: timeRange.begin,
      end: timeRange.end,
    },
    abortSignal
  )) {
    if (result.type === 'batch') {
      batches.push(result.batch)
    } else if (result.type === 'error') {
      throw new Error(result.error.message)
    }
  }

  if (batches.length === 0) {
    // Return empty table
    return new Table()
  }
  return new Table(batches)
}

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
          <Button onClick={onConfirm} className="bg-red-600 hover:bg-red-700">
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
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id })

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
  savedConfig: _savedConfig,
  onUnsavedChange,
  timeRange,
  onSave,
  isSaving,
  hasUnsavedChanges,
  onSaveAs,
  saveError,
  refreshTrigger,
}: ScreenRendererProps) {
  // Cast config to NotebookConfig
  const notebookConfig = useMemo(() => {
    const cfg = config as unknown as NotebookConfig | null
    return cfg && cfg.cells ? cfg : { cells: [] }
  }, [config])

  const cells = notebookConfig.cells

  // Cell execution states
  const [cellStates, setCellStates] = useState<Record<string, CellState>>({})

  // Variable values (collected from variable cells)
  const [variableValues, setVariableValues] = useState<Record<string, string>>({})
  // Ref for synchronous access during sequential execution (state updates are async)
  const variableValuesRef = useRef<Record<string, string>>({})

  // Selected cell index
  const [selectedCellIndex, setSelectedCellIndex] = useState<number | null>(null)

  // Add cell modal
  const [showAddCellModal, setShowAddCellModal] = useState(false)

  // Delete confirmation
  const [deletingCellIndex, setDeletingCellIndex] = useState<number | null>(null)

  // Abort controller for cancelling queries
  const abortControllerRef = useRef<AbortController | null>(null)

  // Get existing cell names for uniqueness check
  const existingNames = useMemo(() => {
    return new Set(cells.map((c) => c.name))
  }, [cells])

  // Drag and drop state
  const [activeDragId, setActiveDragId] = useState<string | null>(null)

  // Drag and drop sensors
  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: {
        distance: 8, // Require 8px movement before starting drag
      },
    }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    })
  )

  // Handle drag start - track active cell
  const handleDragStart = useCallback((event: DragStartEvent) => {
    setActiveDragId(event.active.id as string)
  }, [])

  // Handle drag end - reorder cells
  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      setActiveDragId(null)
      const { active, over } = event
      if (!over || active.id === over.id) return

      const oldIndex = cells.findIndex((c) => c.name === active.id)
      const newIndex = cells.findIndex((c) => c.name === over.id)

      if (oldIndex === -1 || newIndex === -1) return

      const newCells = arrayMove(cells, oldIndex, newIndex)
      onConfigChange({ ...notebookConfig, cells: newCells })
      onUnsavedChange()

      // Update selected cell index if needed
      if (selectedCellIndex === oldIndex) {
        setSelectedCellIndex(newIndex)
      } else if (selectedCellIndex !== null) {
        // Adjust selection if it was affected by the move
        if (oldIndex < selectedCellIndex && newIndex >= selectedCellIndex) {
          setSelectedCellIndex(selectedCellIndex - 1)
        } else if (oldIndex > selectedCellIndex && newIndex <= selectedCellIndex) {
          setSelectedCellIndex(selectedCellIndex + 1)
        }
      }
    },
    [cells, notebookConfig, onConfigChange, onUnsavedChange, selectedCellIndex]
  )

  // Initialize variable values from config defaults
  useEffect(() => {
    const initialValues: Record<string, string> = {}
    for (const cell of cells) {
      if (cell.type === 'variable') {
        const varCell = cell as VariableCellConfig
        if (varCell.defaultValue && !variableValues[cell.name]) {
          initialValues[cell.name] = varCell.defaultValue
        }
      }
    }
    if (Object.keys(initialValues).length > 0) {
      variableValuesRef.current = { ...variableValuesRef.current, ...initialValues }
      setVariableValues((prev) => ({ ...prev, ...initialValues }))
    }
  }, [cells]) // eslint-disable-line react-hooks/exhaustive-deps

  // Execute a single cell
  const executeCell = useCallback(
    async (cellIndex: number): Promise<boolean> => {
      const cell = cells[cellIndex]
      if (!cell) return false

      // Handle markdown cells (no execution needed)
      if (cell.type === 'markdown') {
        setCellStates((prev) => ({
          ...prev,
          [cell.name]: { status: 'success', data: null },
        }))
        return true
      }

      // Mark cell as loading
      setCellStates((prev) => ({
        ...prev,
        [cell.name]: { ...prev[cell.name], status: 'loading', error: undefined, data: null },
      }))

      // Get SQL from cell
      let sql: string | undefined
      if (cell.type === 'variable') {
        const varCell = cell as VariableCellConfig
        sql = varCell.sql
      } else {
        const queryCell = cell as QueryCellConfig
        sql = queryCell.sql
      }

      if (!sql) {
        setCellStates((prev) => ({
          ...prev,
          [cell.name]: { status: 'success', data: null },
        }))
        return true
      }

      // Gather variables from cells above (use ref for synchronous access during execution)
      const availableVariables: Record<string, string> = {}
      for (let i = 0; i < cellIndex; i++) {
        const prevCell = cells[i]
        if (prevCell.type === 'variable' && variableValuesRef.current[prevCell.name] !== undefined) {
          availableVariables[prevCell.name] = variableValuesRef.current[prevCell.name]
        }
      }

      // Substitute macros
      const substitutedSql = substituteMacros(sql, availableVariables, timeRange)

      // Create new abort controller for this execution
      abortControllerRef.current?.abort()
      abortControllerRef.current = new AbortController()

      try {
        const result = await executeSql(substitutedSql, timeRange, abortControllerRef.current.signal)

        // For variable cells, extract options from result
        // Convention: 1 column = value+label, 2 columns = value then label
        if (cell.type === 'variable') {
          const options: { label: string; value: string }[] = []
          if (result && result.numRows > 0 && result.numCols > 0) {
            const schema = result.schema
            const valueColName = schema.fields[0].name
            const labelColName = schema.fields.length > 1 ? schema.fields[1].name : valueColName
            for (let i = 0; i < result.numRows; i++) {
              const row = result.get(i)
              if (row) {
                const value = String(row[valueColName] ?? '')
                const label = String(row[labelColName] ?? value)
                options.push({ label, value })
              }
            }
          }
          setCellStates((prev) => ({
            ...prev,
            [cell.name]: { status: 'success', data: result, variableOptions: options },
          }))
          // Set default value if not already set (update ref synchronously for next cell)
          if (!variableValuesRef.current[cell.name] && options.length > 0) {
            const newValue = options[0].value
            variableValuesRef.current = { ...variableValuesRef.current, [cell.name]: newValue }
            setVariableValues((prev) => ({ ...prev, [cell.name]: newValue }))
          }
        } else {
          setCellStates((prev) => ({
            ...prev,
            [cell.name]: { status: 'success', data: result },
          }))
        }
        return true
      } catch (err) {
        if (err instanceof Error && err.name === 'AbortError') {
          return false
        }
        const errorMessage = err instanceof Error ? err.message : String(err)
        setCellStates((prev) => ({
          ...prev,
          [cell.name]: { status: 'error', error: errorMessage, data: null },
        }))
        return false
      }
    },
    [cells, timeRange]
  )

  // Execute from a cell index (that cell and all below)
  const executeFromCell = useCallback(
    async (startIndex: number) => {
      for (let i = startIndex; i < cells.length; i++) {
        const success = await executeCell(i)
        if (!success) {
          // Mark remaining cells as blocked
          for (let j = i + 1; j < cells.length; j++) {
            const blockedCell = cells[j]
            if (blockedCell.type !== 'markdown') {
              setCellStates((prev) => ({
                ...prev,
                [blockedCell.name]: { status: 'blocked', data: null },
              }))
            }
          }
          break
        }
      }
    },
    [cells, executeCell]
  )

  // Execute all cells on initial load
  const hasExecutedRef = useRef(false)
  useEffect(() => {
    if (!hasExecutedRef.current && cells.length > 0) {
      hasExecutedRef.current = true
      executeFromCell(0)
    }
  }, []) // eslint-disable-line react-hooks/exhaustive-deps

  // Re-execute on refresh trigger
  const prevRefreshRef = useRef(refreshTrigger)
  useEffect(() => {
    if (prevRefreshRef.current !== refreshTrigger) {
      prevRefreshRef.current = refreshTrigger
      executeFromCell(0)
    }
  }, [refreshTrigger, executeFromCell])

  // Add a new cell
  const handleAddCell = useCallback(
    (type: CellType) => {
      const newCell = createDefaultCell(type, existingNames)
      const newCells = [...cells, newCell]
      onConfigChange({ ...notebookConfig, cells: newCells })
      onUnsavedChange()
      setShowAddCellModal(false)
      // Select the new cell
      setSelectedCellIndex(newCells.length - 1)
    },
    [notebookConfig, cells, existingNames, onConfigChange, onUnsavedChange]
  )

  // Delete a cell
  const handleDeleteCell = useCallback(
    (index: number) => {
      const cell = cells[index]
      const newCells = cells.filter((_, i) => i !== index)
      onConfigChange({ ...notebookConfig, cells: newCells })
      onUnsavedChange()
      // Remove cell state
      setCellStates((prev) => {
        const next = { ...prev }
        delete next[cell.name]
        return next
      })
      // Remove variable value if applicable
      if (cell.type === 'variable') {
        const nextRef = { ...variableValuesRef.current }
        delete nextRef[cell.name]
        variableValuesRef.current = nextRef
        setVariableValues((prev) => {
          const next = { ...prev }
          delete next[cell.name]
          return next
        })
      }
      // Clear selection if deleted cell was selected
      if (selectedCellIndex === index) {
        setSelectedCellIndex(null)
      } else if (selectedCellIndex !== null && selectedCellIndex > index) {
        setSelectedCellIndex(selectedCellIndex - 1)
      }
      setDeletingCellIndex(null)
    },
    [notebookConfig, cells, onConfigChange, onUnsavedChange, selectedCellIndex]
  )

  // Update cell config using functional update to ensure atomic operations on latest state
  const updateCell = useCallback(
    (index: number, updates: Partial<CellConfig>) => {
      // Use functional update to always operate on the current config (MVC: update model atomically)
      onConfigChange((prev) => {
        const prevNotebook = (prev as unknown as NotebookConfig) || { cells: [] }
        const currentCells = prevNotebook.cells || []
        const cell = currentCells[index]
        if (!cell) return prev // Guard against invalid index

        const newCells = [...currentCells]
        newCells[index] = { ...cell, ...updates } as CellConfig

        // If renaming any cell, migrate execution state to new name
        if (updates.name && updates.name !== cell.name) {
          const oldName = cell.name
          const newName = updates.name
          setCellStates((prevStates) => {
            const next = { ...prevStates }
            if (oldName in next) {
              next[newName] = next[oldName]
              delete next[oldName]
            }
            return next
          })
          // For variable cells, also migrate the stored value
          if (cell.type === 'variable') {
            const nextRef = { ...variableValuesRef.current }
            if (oldName in nextRef) {
              nextRef[newName] = nextRef[oldName]
              delete nextRef[oldName]
              variableValuesRef.current = nextRef
            }
            setVariableValues((prevValues) => {
              const next = { ...prevValues }
              if (oldName in next) {
                next[newName] = next[oldName]
                delete next[oldName]
              }
              return next
            })
          }
        }

        return { ...prevNotebook, cells: newCells }
      })
      onUnsavedChange()
    },
    [onConfigChange, onUnsavedChange]
  )

  // Toggle cell collapsed
  const toggleCellCollapsed = useCallback(
    (index: number) => {
      const cell = cells[index]
      updateCell(index, {
        layout: { ...cell.layout, collapsed: !cell.layout.collapsed },
      })
    },
    [cells, updateCell]
  )

  // Handle variable value change
  const handleVariableChange = useCallback(
    (cellName: string, value: string) => {
      variableValuesRef.current = { ...variableValuesRef.current, [cellName]: value }
      setVariableValues((prev) => ({ ...prev, [cellName]: value }))
    },
    []
  )

  // Get the selected cell
  const selectedCell = selectedCellIndex !== null ? cells[selectedCellIndex] : null

  // Render a cell
  const renderCell = (cell: CellConfig, index: number) => {
    const state = cellStates[cell.name] || { status: 'idle', data: null }
    const CellRenderer = getCellRenderer(cell.type)

    // Gather variables available to this cell (from cells above)
    const availableVariables: Record<string, string> = {}
    for (let i = 0; i < index; i++) {
      const prevCell = cells[i]
      if (prevCell.type === 'variable' && variableValues[prevCell.name] !== undefined) {
        availableVariables[prevCell.name] = variableValues[prevCell.name]
      }
    }

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
            statusText={state.data ? `${state.data.numRows} rows` : undefined}
            height={cell.layout.height}
          >
            {CellRenderer ? (
              <CellRenderer
                name={cell.name}
                sql={cell.type !== 'markdown' ? (cell as QueryCellConfig | VariableCellConfig).sql : undefined}
                options={cell.type !== 'markdown' && cell.type !== 'variable' ? (cell as QueryCellConfig).options : undefined}
                data={state.data}
                status={state.status}
                error={state.error}
                timeRange={timeRange}
                variables={availableVariables}
                isEditing={selectedCellIndex === index}
                onRun={() => executeCell(index)}
                onSqlChange={(sql) => updateCell(index, { sql } as Partial<QueryCellConfig>)}
                onOptionsChange={(options) => updateCell(index, { options } as Partial<QueryCellConfig>)}
                content={cell.type === 'markdown' ? (cell as MarkdownCellConfig).content : undefined}
                onContentChange={
                  cell.type === 'markdown'
                    ? (content) => updateCell(index, { content } as Partial<MarkdownCellConfig>)
                    : undefined
                }
                value={cell.type === 'variable' ? variableValues[cell.name] : undefined}
                onValueChange={
                  cell.type === 'variable'
                    ? (value) => handleVariableChange(cell.name, value)
                    : undefined
                }
                variableType={cell.type === 'variable' ? (cell as VariableCellConfig).variableType : undefined}
                variableOptions={cell.type === 'variable' ? state.variableOptions : undefined}
              />
            ) : (
              <div className="text-theme-text-muted">
                No renderer for cell type: {cell.type}
              </div>
            )}
          </CellContainer>
        )}
      </SortableCell>
    )
  }

  return (
    <div className="flex h-full">
      {/* Main content area */}
      <div className="flex-1 flex flex-col p-6 min-w-0 overflow-auto">
        {/* Cells */}
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragStart={handleDragStart}
          onDragEnd={handleDragEnd}
        >
          <SortableContext
            items={cells.map((c) => c.name)}
            strategy={verticalListSortingStrategy}
          >
            <div className="flex flex-col gap-3">
              {cells.map((cell, index) => renderCell(cell, index))}

              {/* Add Cell button */}
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
        <div className="w-[350px] bg-app-panel border-l border-theme-border flex flex-col flex-shrink-0">
          <CellEditor
            cell={selectedCell}
            variables={variableValues}
            timeRange={timeRange}
            onClose={() => setSelectedCellIndex(null)}
            onUpdate={(updates) => updateCell(selectedCellIndex!, updates)}
            onRun={() => executeCell(selectedCellIndex!)}
            onDelete={() => setDeletingCellIndex(selectedCellIndex!)}
          />
          <div className="border-t border-theme-border">
            <SaveFooter
              onSave={onSave}
              onSaveAs={onSaveAs}
              isSaving={isSaving}
              hasUnsavedChanges={hasUnsavedChanges}
              saveError={saveError}
            />
          </div>
        </div>
      )}

      {/* Add Cell Modal */}
      <AddCellModal
        isOpen={showAddCellModal}
        onClose={() => setShowAddCellModal(false)}
        onAdd={handleAddCell}
      />

      {/* Delete Confirmation Modal */}
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
