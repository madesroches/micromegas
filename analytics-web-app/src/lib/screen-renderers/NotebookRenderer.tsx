import { useState, useCallback, useMemo, useEffect } from 'react'
import { useSearchParams } from 'react-router-dom'
import { Plus, X, Trash2 } from 'lucide-react'
import {
  DndContext,
  closestCenter,
  DragOverlay,
} from '@dnd-kit/core'
import {
  SortableContext,
  useSortable,
} from '@dnd-kit/sortable'
import { CSS } from '@dnd-kit/utilities'
import { registerRenderer, ScreenRendererProps } from './index'
import {
  getCellRenderer,
  getCellTypeMetadata,
} from './cell-registry'
import type { CellConfig, VariableCellConfig, NotebookConfig, HorizontalGroupCellConfig, VariableValue } from './notebook-types'
import { CellContainer } from '@/components/CellContainer'
import { CellEditor } from '@/components/CellEditor'
import { ResizeHandle } from '@/components/ResizeHandle'
import { Button } from '@/components/ui/button'
import { useNotebookVariables } from './useNotebookVariables'
import { useCellExecution } from './useCellExecution'
import { cleanupVariableParams, resolveCellDataSource, flattenCellsForExecution, collectAllCellNames, validateCellName, sanitizeCellName } from './notebook-utils'
import { HorizontalGroupCell, HorizontalGroupCellEditor } from './cells/HorizontalGroupCell'
import { cleanupTimeParams, useExposeSaveRef } from '@/lib/url-cleanup-utils'
import { getTimeRangeForApi } from '@/lib/time-range'
import { buildCellRendererProps, buildStatusText, buildHgStatusText, computeHgStatus } from './notebook-cell-view'
import { AddCellModal } from './shared'
import { useWasmEngine } from './useWasmEngine'
import { useEditorPanelWidth } from './useEditorPanelWidth'
import { NotebookSourceView } from './NotebookSourceView'
import { useNotebookAutoRun } from './useNotebookAutoRun'
import { useTimeRangeSync } from './useTimeRangeSync'
import { useCellSortCheck } from './useCellSortCheck'
import { useNotebookDragDrop } from './useNotebookDragDrop'
import { useCellManager } from './useCellManager'

// ============================================================================
// Modal Components
// ============================================================================

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
  onChildRun: (childName: string) => void
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  allCellNames: Set<string>
  defaultDataSource?: string
  datasourceVariables?: string[]
  showNotebookOption?: boolean
  onClose: () => void
  onUpdate: (updates: Partial<CellConfig>) => void
  onDelete: () => void
}

function HgEditorPanel({
  config,
  selectedChildName,
  onChildSelect,
  onChildRun,
  variables,
  timeRange,
  allCellNames,
  defaultDataSource,
  datasourceVariables,
  showNotebookOption,
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
        {/* Group Name — only when not editing a child */}
        {!selectedChildName && (
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
        )}
        {/* Children management */}
        <HorizontalGroupCellEditor
          config={config}
          onChange={(newConfig) => onUpdate(newConfig)}
          selectedChildName={selectedChildName}
          onChildSelect={onChildSelect}
          onChildRun={onChildRun}
          variables={variables}
          timeRange={timeRange}
          allCellNames={allCellNames}
          defaultDataSource={defaultDataSource}
          datasourceVariables={datasourceVariables}
          showNotebookOption={showNotebookOption}
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
  onExecutingChange,
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
  useTimeRangeSync({ rawTimeRange, config: notebookConfig, onConfigChange })

  // Variable values management - hook owns URL access for variables
  const { variableValues, variableValuesRef, setVariableValue, migrateVariable, removeVariable } =
    useNotebookVariables(
      cells,
      savedNotebookConfig?.cells ?? null,
    )

  // WASM engine for notebook-local queries
  const { engine, engineError } = useWasmEngine()

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

  // Report execution state to parent
  const isExecuting = useMemo(
    () => Object.values(cellStates).some((s) => s.status === 'loading'),
    [cellStates]
  )
  useEffect(() => { onExecutingChange?.(isExecuting) }, [isExecuting, onExecutingChange])

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

  // Auto-run management
  const { scheduleAutoRun, triggerAutoRun } = useNotebookAutoRun({ executeFromCellByName })

  // Handle time range selection from charts (drag-to-zoom)
  const handleTimeRangeSelect = useCallback((from: Date, to: Date) => {
    onTimeRangeChange(from.toISOString(), to.toISOString())
  }, [onTimeRangeChange])

  // Re-execute table cells when sort options change
  useCellSortCheck({ executionCells, executeCellByName })

  // UI state
  const [selectedCellIndex, setSelectedCellIndex] = useState<number | null>(null)
  const [selectedChildName, setSelectedChildName] = useState<string | null>(null)
  const [showAddCellModal, setShowAddCellModal] = useState(false)
  const [deletingCellIndex, setDeletingCellIndex] = useState<number | null>(null)
  const [showSource, setShowSource] = useState(false)
  const handleCloseSource = useCallback(() => setShowSource(false), [])

  // Editor panel width
  const { editorPanelWidth, handleEditorPanelResize } = useEditorPanelWidth()

  // Existing cell names for uniqueness check (includes hg children)
  const existingNames = useMemo(() => collectAllCellNames(cells), [cells])

  // Drag and drop
  const {
    sensors,
    hgAwareSortingStrategy,
    handleDragStart,
    handleDragOver,
    handleDragEnd,
    activeDragId,
    dragOverZone,
    dragOverHgName,
  } = useNotebookDragDrop({
    cells,
    notebookConfig,
    onConfigChange,
    selectedCellIndex,
    setSelectedCellIndex,
    setSelectedChildName,
  })

  // Cell management
  const {
    handleAddCell,
    handleDeleteCell,
    handleDuplicateCell,
    updateCell,
    toggleCellCollapsed,
  } = useCellManager({
    cells,
    notebookConfig,
    existingNames,
    onConfigChange,
    removeCellState,
    migrateCellState,
    setVariableValue,
    migrateVariable,
    removeVariable,
    scheduleAutoRun,
    selectedCellIndex,
    setSelectedCellIndex,
    setSelectedChildName,
    setShowAddCellModal,
    setDeletingCellIndex,
    defaultDataSource: dataSource,
  })

  // Render
  const selectedCell = selectedCellIndex !== null ? cells[selectedCellIndex] : null

  const datasourceVariables = useMemo(() => {
    if (selectedCellIndex === null) return undefined
    return cells
      .slice(0, selectedCellIndex)
      .filter((c) => c.type === 'variable' && (c as VariableCellConfig).variableType === 'datasource')
      .map((c) => c.name)
  }, [cells, selectedCellIndex])

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
      const hgStatusText = buildHgStatusText(hgConfig.children, cellStates)
      const hgStatus = computeHgStatus(hgConfig.children, cellStates)
      return (
        <SortableCell key={cell.name} id={cell.name}>
          {({ dragHandleProps, isDragging, setNodeRef, style }) => (
            <div ref={setNodeRef} style={style} className={dropZoneClass}>
              <CellContainer
                dragHandleProps={dragHandleProps}
                isDragging={isDragging}
                name={cell.name}
                type={cell.type}
                status={hgStatus}
                statusText={hgStatusText}
                collapsed={cell.layout.collapsed}
                childNames={hgConfig.children.map(c => c.name)}
                onToggleCollapsed={() => toggleCellCollapsed(index)}
                isSelected={selectedCellIndex === index}
                onSelect={() => {
                  setSelectedCellIndex(index)
                  setSelectedChildName(null)
                }}
                canRun={true}
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
                    const child = hgConfig.children.find((c) => c.name === cellName)
                    triggerAutoRun(cellName, child?.autoRunFromHere)
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
                    const insertIndex = position === 'before' ? index : index + 1
                    newCells.splice(insertIndex, 0, child)
                    onConfigChange({ ...notebookConfig, cells: newCells })
                    if (selectedChildName === childName) {
                      setSelectedChildName(null)
                    }
                  }}
                  onTimeRangeSelect={handleTimeRangeSelect}
                  defaultDataSource={dataSource}
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

    const statusText = buildStatusText(cell, state)
    const cellDataSource = resolveCellDataSource(cell, availableVariables, dataSource)

    const commonRendererProps = buildCellRendererProps(cell, state,
      {
        availableVariables,
        allVariableValues: variableValues,
        timeRange: getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to),
        isEditing: selectedCellIndex === index,
        dataSource: cellDataSource,
      },
      {
        onRun: () => executeCellByName(cell.name),
        onSqlChange: (sql: string) => updateCell(index, { sql }),
        onOptionsChange: (options: Record<string, unknown>) => updateCell(index, { options }),
        onContentChange: (content: string) => updateCell(index, { content }),
        onValueChange: (value: VariableValue) => {
          setVariableValue(cell.name, value)
          triggerAutoRun(cell.name, cell.autoRunFromHere)
        },
        onTimeRangeSelect: handleTimeRangeSelect,
      },
    )

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
      <div className="flex-1 flex flex-col p-2 min-w-0 overflow-auto">
        {engineError && (
          <div className="mb-3 px-4 py-2 bg-accent-error/10 border border-accent-error/30 rounded text-xs text-accent-error">
            WASM engine failed to load: {engineError}
          </div>
        )}
        {showSource ? (
          <NotebookSourceView
            notebookConfig={notebookConfig}
            onConfigChange={onConfigChange}
            onBack={handleCloseSource}
          />
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
                <div className="flex flex-col gap-1">
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
                onChildRun={(childName) => executeCellByName(childName)}
                variables={variableValues}
                timeRange={getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to)}
                allCellNames={existingNames}
                defaultDataSource={dataSource}
                showNotebookOption
                datasourceVariables={datasourceVariables}
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
                datasourceVariables={datasourceVariables}
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
