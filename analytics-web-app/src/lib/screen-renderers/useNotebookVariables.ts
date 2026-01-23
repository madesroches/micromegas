import { useCallback, useMemo, useRef, useEffect } from 'react'
import { CellConfig, VariableCellConfig } from './notebook-utils'

export interface UseNotebookVariablesResult {
  /** Current variable values (merged from URL config and cell defaults) */
  variableValues: Record<string, string>
  /** Ref for synchronous access during sequential cell execution */
  variableValuesRef: React.MutableRefObject<Record<string, string>>
  /** Set a variable value (calls onVariableChange callback) */
  setVariableValue: (cellName: string, value: string) => void
  /** Migrate variable state when a cell is renamed (no-op, state is in URL) */
  migrateVariable: (oldName: string, newName: string) => void
  /** Remove variable state when a cell is deleted (no-op, state is in URL) */
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
  onVariableChange?: (name: string, value: string) => void
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

  // Migration is a no-op since state lives in URL, not in this hook
  // When a variable cell is renamed, the old URL param becomes orphaned
  // and the new name will use its default value until set
  const migrateVariable = useCallback((_oldName: string, _newName: string) => {
    // No-op: URL params are keyed by name, migration happens naturally
  }, [])

  // Removal is a no-op since state lives in URL, not in this hook
  // Orphaned URL params are simply ignored
  const removeVariable = useCallback((_cellName: string) => {
    // No-op: URL params for deleted variables are ignored
  }, [])

  return {
    variableValues,
    variableValuesRef,
    setVariableValue,
    migrateVariable,
    removeVariable,
  }
}
