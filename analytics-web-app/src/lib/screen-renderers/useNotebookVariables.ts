import { useCallback, useMemo, useRef, useEffect } from 'react'
import { CellConfig, VariableCellConfig } from './notebook-utils'

export interface UseNotebookVariablesResult {
  /** Current variable values (merged from URL config and cell defaults) */
  variableValues: Record<string, string>
  /** Ref for synchronous access during sequential cell execution */
  variableValuesRef: React.MutableRefObject<Record<string, string>>
  /** Set a variable value (calls onVariableChange callback) */
  setVariableValue: (cellName: string, value: string) => void
  /** Migrate variable state when a cell is renamed (transfers value, removes old param) */
  migrateVariable: (oldName: string, newName: string) => void
  /** Remove variable state when a cell is deleted (removes URL param) */
  removeVariable: (cellName: string) => void
}

/**
 * Manages variable values for notebook cells.
 *
 * Variables are collected from variable cells and can be referenced in SQL queries
 * of cells below them. This hook:
 * - Computes effective values from URL config (source of truth) + cell defaults
 * - Provides synchronous ref access for sequential execution
 * - Delegates state changes to the parent via onVariableChange callback
 *
 * The URL config is the single source of truth. This hook does NOT own state;
 * it computes effective values by merging URL config with cell defaults.
 */
export function useNotebookVariables(
  cells: CellConfig[],
  configVariables: Record<string, string> = {},
  onVariableChange?: (name: string, value: string) => void,
  onVariableRemove?: (name: string) => void
): UseNotebookVariablesResult {
  // Compute effective values: config value → defaultValue → undefined
  const variableValues = useMemo(() => {
    const values: Record<string, string> = { ...configVariables }

    // Apply defaults for variables not in config
    for (const cell of cells) {
      if (cell.type === 'variable' && !(cell.name in values)) {
        const varCell = cell as VariableCellConfig
        if (varCell.defaultValue) {
          values[cell.name] = varCell.defaultValue
        }
      }
    }
    return values
  }, [cells, configVariables])

  // Ref for synchronous access during sequential execution
  const variableValuesRef = useRef<Record<string, string>>(variableValues)

  // Keep ref in sync with computed values
  useEffect(() => {
    variableValuesRef.current = variableValues
  }, [variableValues])

  // Set a variable value - delegates to callback
  const setVariableValue = useCallback(
    (cellName: string, value: string) => {
      // Update ref immediately for synchronous access during execution
      variableValuesRef.current = { ...variableValuesRef.current, [cellName]: value }
      // Delegate to parent callback (updates URL config)
      onVariableChange?.(cellName, value)
    },
    [onVariableChange]
  )

  // Migrate variable from old name to new name when cell is renamed
  const migrateVariable = useCallback(
    (oldName: string, newName: string) => {
      const oldValue = variableValuesRef.current[oldName]
      if (oldValue !== undefined) {
        // Update ref
        const nextRef = { ...variableValuesRef.current }
        nextRef[newName] = oldValue
        delete nextRef[oldName]
        variableValuesRef.current = nextRef
        // Update URL: set new name, remove old name
        onVariableChange?.(newName, oldValue)
        onVariableRemove?.(oldName)
      }
    },
    [onVariableChange, onVariableRemove]
  )

  // Remove variable from URL when cell is deleted
  const removeVariable = useCallback(
    (cellName: string) => {
      // Remove from ref immediately
      const nextRef = { ...variableValuesRef.current }
      delete nextRef[cellName]
      variableValuesRef.current = nextRef
      // Delegate to parent callback (updates URL config)
      onVariableRemove?.(cellName)
    },
    [onVariableRemove]
  )

  return {
    variableValues,
    variableValuesRef,
    setVariableValue,
    migrateVariable,
    removeVariable,
  }
}
