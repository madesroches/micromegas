import { useCallback } from 'react'
import { CellType, createDefaultCell } from './cell-registry'
import type { CellConfig, VariableCellConfig, NotebookConfig, HorizontalGroupCellConfig, VariableValue } from './notebook-types'

interface UseCellManagerParams {
  cells: CellConfig[]
  notebookConfig: NotebookConfig
  existingNames: Set<string>
  onConfigChange: (config: NotebookConfig) => void
  // Execution state management
  removeCellState: (name: string) => void
  migrateCellState: (oldName: string, newName: string) => void
  // Variable management
  setVariableValue: (cellName: string, value: VariableValue) => void
  migrateVariable: (oldName: string, newName: string) => void
  removeVariable: (cellName: string) => void
  // Auto-run scheduling
  scheduleAutoRun: (cellName: string) => void
  // Selection updates
  selectedCellIndex: number | null
  setSelectedCellIndex: (index: number | null) => void
  setSelectedChildName: (name: string | null) => void
  setShowAddCellModal: (show: boolean) => void
  setDeletingCellIndex: (index: number | null) => void
  defaultDataSource?: string
}

interface UseCellManagerResult {
  handleAddCell: (type: CellType) => void
  handleInsertCell: (type: CellType, atIndex: number) => void
  handleDeleteCell: (index: number) => void
  handleDuplicateCell: (index: number) => void
  updateCell: (index: number, updates: Partial<CellConfig>) => void
  toggleCellCollapsed: (index: number) => void
}

/**
 * Manages cell CRUD operations for notebooks.
 *
 * Owns:
 * - Adding new cells with unique name generation
 * - Deleting cells with state cleanup (execution state, variables)
 * - Duplicating cells with name uniquification (including hg children)
 * - Updating cells with rename migration, hg child tracking, and auto-run scheduling
 * - Toggling cell collapsed state
 */
export function useCellManager({
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
  defaultDataSource,
}: UseCellManagerParams): UseCellManagerResult {
  const handleAddCell = useCallback(
    (type: CellType) => {
      const newCell = createDefaultCell(type, existingNames, defaultDataSource)
      const newCells = [...cells, newCell]
      const newConfig = { ...notebookConfig, cells: newCells }
      onConfigChange(newConfig)
      setShowAddCellModal(false)
      setSelectedCellIndex(newCells.length - 1)
    },
    [notebookConfig, cells, existingNames, onConfigChange, setShowAddCellModal, setSelectedCellIndex, defaultDataSource]
  )

  const handleInsertCell = useCallback(
    (type: CellType, atIndex: number) => {
      const newCell = createDefaultCell(type, existingNames, defaultDataSource)
      const newCells = [...cells.slice(0, atIndex), newCell, ...cells.slice(atIndex)]
      const newConfig = { ...notebookConfig, cells: newCells }
      onConfigChange(newConfig)
      setSelectedCellIndex(atIndex)
    },
    [notebookConfig, cells, existingNames, onConfigChange, setSelectedCellIndex, defaultDataSource]
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
    [notebookConfig, cells, onConfigChange, selectedCellIndex, removeCellState, removeVariable, setSelectedCellIndex, setSelectedChildName, setDeletingCellIndex]
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
    [cells, existingNames, notebookConfig, onConfigChange, setSelectedCellIndex]
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

      // Handle hg child changes: clean up removed children, migrate renamed children
      if (cell.type === 'hg' && 'children' in updates) {
        const oldChildren = (cell as HorizontalGroupCellConfig).children
        const newChildren = (updates as Partial<HorizontalGroupCellConfig>).children || []
        const newNames = new Set(newChildren.map(c => c.name))

        for (const oldChild of oldChildren) {
          if (newNames.has(oldChild.name)) continue

          // Old child name not in new list — check if renamed (same index, same type)
          const oldIdx = oldChildren.indexOf(oldChild)
          const newChild = newChildren[oldIdx]
          if (
            newChild &&
            newChild.type === oldChild.type &&
            !oldChildren.some(c => c.name === newChild.name)
          ) {
            // Rename: migrate state to new name
            migrateCellState(oldChild.name, newChild.name)
            if (oldChild.type === 'variable') {
              migrateVariable(oldChild.name, newChild.name)
            }
          } else {
            // Deletion: clean up state
            removeCellState(oldChild.name)
            if (oldChild.type === 'variable') {
              removeVariable(oldChild.name)
            }
          }
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
    [cells, notebookConfig, onConfigChange, migrateCellState, migrateVariable, setVariableValue, removeCellState, removeVariable, scheduleAutoRun]
  )

  const toggleCellCollapsed = useCallback(
    (index: number) => {
      const cell = cells[index]
      updateCell(index, { layout: { ...cell.layout, collapsed: !cell.layout.collapsed } })
    },
    [cells, updateCell]
  )

  return {
    handleAddCell,
    handleInsertCell,
    handleDeleteCell,
    handleDuplicateCell,
    updateCell,
    toggleCellCollapsed,
  }
}
