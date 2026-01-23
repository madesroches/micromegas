import { useState, useCallback, useEffect, useRef } from 'react'
import { CellConfig, VariableCellConfig } from './notebook-utils'

export interface VariableState {
  /** Current variable values (cellName -> value) */
  values: Record<string, string>
  /** Ref for synchronous access during sequential execution */
  valuesRef: React.MutableRefObject<Record<string, string>>
}

export interface UseNotebookVariablesResult {
  /** Current variable values */
  variableValues: Record<string, string>
  /** Ref for synchronous access during sequential cell execution */
  variableValuesRef: React.MutableRefObject<Record<string, string>>
  /** Set a variable value (updates both state and ref) */
  setVariableValue: (cellName: string, value: string) => void
  /** Migrate variable state when a cell is renamed */
  migrateVariable: (oldName: string, newName: string) => void
  /** Remove variable state when a cell is deleted */
  removeVariable: (cellName: string) => void
}

/**
 * Manages variable values for notebook cells.
 *
 * Variables are collected from variable cells and can be referenced in SQL queries
 * of cells below them. This hook handles:
 * - State initialization from cell default values
 * - Synchronous ref access for sequential execution
 * - State migration on cell rename
 * - State cleanup on cell delete
 */
export function useNotebookVariables(cells: CellConfig[]): UseNotebookVariablesResult {
  const [variableValues, setVariableValues] = useState<Record<string, string>>({})
  const variableValuesRef = useRef<Record<string, string>>({})

  // Initialize variable values from config defaults
  useEffect(() => {
    const initialValues: Record<string, string> = {}
    for (const cell of cells) {
      if (cell.type === 'variable') {
        const varCell = cell as VariableCellConfig
        if (varCell.defaultValue && !variableValuesRef.current[cell.name]) {
          initialValues[cell.name] = varCell.defaultValue
        }
      }
    }
    if (Object.keys(initialValues).length > 0) {
      variableValuesRef.current = { ...variableValuesRef.current, ...initialValues }
      setVariableValues((prev) => ({ ...prev, ...initialValues }))
    }
  }, [cells])

  // Set a variable value (updates both state and ref synchronously)
  const setVariableValue = useCallback((cellName: string, value: string) => {
    variableValuesRef.current = { ...variableValuesRef.current, [cellName]: value }
    setVariableValues((prev) => ({ ...prev, [cellName]: value }))
  }, [])

  // Migrate variable state when a cell is renamed
  const migrateVariable = useCallback((oldName: string, newName: string) => {
    const nextRef = { ...variableValuesRef.current }
    if (oldName in nextRef) {
      nextRef[newName] = nextRef[oldName]
      delete nextRef[oldName]
      variableValuesRef.current = nextRef
    }
    setVariableValues((prev) => {
      const next = { ...prev }
      if (oldName in next) {
        next[newName] = next[oldName]
        delete next[oldName]
      }
      return next
    })
  }, [])

  // Remove variable state when a cell is deleted
  const removeVariable = useCallback((cellName: string) => {
    const nextRef = { ...variableValuesRef.current }
    delete nextRef[cellName]
    variableValuesRef.current = nextRef
    setVariableValues((prev) => {
      const next = { ...prev }
      delete next[cellName]
      return next
    })
  }, [])

  return {
    variableValues,
    variableValuesRef,
    setVariableValue,
    migrateVariable,
    removeVariable,
  }
}
