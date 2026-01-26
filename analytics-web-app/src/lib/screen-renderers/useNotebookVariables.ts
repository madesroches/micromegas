import { useCallback, useMemo, useRef, useEffect } from 'react'
import { CellConfig, VariableCellConfig } from './notebook-utils'

export interface UseNotebookVariablesResult {
  /** Current variable values (merged from URL config and cell defaults) */
  variableValues: Record<string, string>
  /** Ref for synchronous access during sequential cell execution */
  variableValuesRef: React.MutableRefObject<Record<string, string>>
  /** Set a variable value (calls onVariableChange callback, uses delta logic against saved baseline) */
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
 * - Computes effective values from saved defaults + URL overrides (delta from saved)
 * - Provides synchronous ref access for sequential execution
 * - Delegates state changes to the parent via onVariableChange callback
 * - Uses delta-based URL updates: URL only contains values different from saved baseline
 *
 * The URL config contains deltas from saved values. Effective values are computed by:
 * 1. Starting with saved defaults (or current cell defaults for unsaved variables)
 * 2. Overriding with URL values (which represent user changes from saved state)
 */
export function useNotebookVariables(
  cells: CellConfig[],
  savedCells: CellConfig[] | null | undefined,
  configVariables: Record<string, string> = {},
  onVariableChange?: (name: string, value: string) => void,
  onVariableRemove?: (name: string) => void
): UseNotebookVariablesResult {
  // Build a lookup map for saved defaults (O(1) access)
  const savedDefaultsByName = useMemo(() => {
    const map = new Map<string, string>()
    if (savedCells) {
      for (const cell of savedCells) {
        if (cell.type === 'variable') {
          const varCell = cell as VariableCellConfig
          if (varCell.defaultValue !== undefined) {
            map.set(cell.name, varCell.defaultValue)
          }
        }
      }
    }
    return map
  }, [savedCells])
  // Compute effective values: saved default → current default → URL override
  // URL values represent deltas from saved baseline
  const variableValues = useMemo(() => {
    const values: Record<string, string> = {}

    // Start with baseline values for all known variables
    // Priority: saved default → current cell default
    for (const cell of cells) {
      if (cell.type === 'variable') {
        const varCell = cell as VariableCellConfig
        // Use saved default if available, otherwise use current cell's default
        const savedDefault = savedDefaultsByName.get(cell.name)
        const baseline = savedDefault ?? varCell.defaultValue
        if (baseline !== undefined) {
          values[cell.name] = baseline
        }
      }
    }

    // Override with URL values (these are the deltas from saved state)
    for (const [name, value] of Object.entries(configVariables)) {
      values[name] = value
    }

    return values
  }, [cells, configVariables, savedDefaultsByName])

  // Ref for synchronous access during sequential execution
  const variableValuesRef = useRef<Record<string, string>>(variableValues)

  // Keep ref in sync with computed values
  useEffect(() => {
    variableValuesRef.current = variableValues
  }, [variableValues])

  // Set a variable value - uses delta logic against saved baseline
  const setVariableValue = useCallback(
    (cellName: string, value: string) => {
      // Update ref immediately for synchronous access during execution
      variableValuesRef.current = { ...variableValuesRef.current, [cellName]: value }

      // Determine baseline: saved default → current cell default
      const savedDefault = savedDefaultsByName.get(cellName)
      const currentCell = cells.find((c) => c.type === 'variable' && c.name === cellName) as
        | VariableCellConfig
        | undefined
      const baseline = savedDefault ?? currentCell?.defaultValue

      // Delta logic: only add to URL if different from baseline
      if (value === baseline) {
        // Value matches baseline - remove from URL
        onVariableRemove?.(cellName)
      } else {
        // Value differs from baseline - add to URL
        onVariableChange?.(cellName, value)
      }
    },
    [cells, savedDefaultsByName, onVariableChange, onVariableRemove]
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
