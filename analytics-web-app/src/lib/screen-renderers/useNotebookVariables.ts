import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { RESERVED_URL_PARAMS } from '@/lib/url-cleanup-utils'
import {
  CellConfig,
  VariableCellConfig,
  VariableValue,
  serializeVariableValue,
  deserializeVariableValue,
  variableValuesEqual,
  flattenCellsForExecution,
} from './notebook-utils'

export interface UseNotebookVariablesResult {
  /** Current variable values (merged from URL config and cell defaults) */
  variableValues: Record<string, VariableValue>
  /** Ref for synchronous access during sequential cell execution */
  variableValuesRef: React.MutableRefObject<Record<string, VariableValue>>
  /** Set a variable value (uses delta logic against saved baseline) */
  setVariableValue: (cellName: string, value: VariableValue) => void
  /** Migrate variable state when a cell is renamed (transfers value, removes old param) */
  migrateVariable: (oldName: string, newName: string) => void
  /** Remove variable state when a cell is deleted (removes URL param) */
  removeVariable: (cellName: string) => void
}

/**
 * Compute variable values from cells + URL overrides.
 * Used once at mount to seed state from the URL.
 */
function computeVariableValues(
  cells: CellConfig[],
  savedDefaultsByName: Map<string, VariableValue>,
  searchParams: URLSearchParams,
): Record<string, VariableValue> {
  const values: Record<string, VariableValue> = {}
  const allCells = flattenCellsForExecution(cells)

  // Start with baseline values for all known variables
  for (const cell of allCells) {
    if (cell.type === 'variable') {
      const varCell = cell as VariableCellConfig
      const savedDefault = savedDefaultsByName.get(cell.name)
      const baseline = savedDefault ?? varCell.defaultValue
      if (baseline !== undefined) {
        values[cell.name] = baseline
      } else if (varCell.variableType === 'text') {
        values[cell.name] = ''
      }
    }
  }

  // Override with URL values (these are deltas from saved state)
  const knownVariableNames = new Set(
    allCells.filter((c) => c.type === 'variable').map((c) => c.name)
  )
  searchParams.forEach((value, key) => {
    if (!RESERVED_URL_PARAMS.has(key) && knownVariableNames.has(key)) {
      values[key] = deserializeVariableValue(value)
    }
  })

  return values
}

/**
 * Manages variable values for notebook cells.
 *
 * URL parameters are read once at mount to seed state, then only written to.
 * After mount, variableValues is local React state — not derived from the URL.
 * This avoids stale-closure bugs when multiple setVariableValue calls happen
 * in the same tick (e.g. user changes datasource → auto-run evaluates expression).
 *
 * The URL reflects local state for bookmarkability and sharing. It stores deltas
 * from saved defaults so that URLs stay short.
 */
export function useNotebookVariables(
  cells: CellConfig[],
  savedCells: CellConfig[] | null | undefined,
): UseNotebookVariablesResult {
  const [searchParams, setSearchParams] = useSearchParams()

  // Build a lookup map for saved defaults (O(1) access)
  const savedDefaultsByName = useMemo(() => {
    const map = new Map<string, VariableValue>()
    if (savedCells) {
      const allCells = flattenCellsForExecution(savedCells)
      for (const cell of allCells) {
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

  // Local state: seeded from URL + defaults on mount, then only updated locally.
  // URL is written to as a side effect but never read back.
  const [variableValues, setVariableValues] = useState(() =>
    computeVariableValues(cells, savedDefaultsByName, searchParams)
  )

  // Ref for synchronous access during sequential cell execution
  const variableValuesRef = useRef<Record<string, VariableValue>>(variableValues)

  // Sync cell list changes into state: when cells are added/removed in the
  // editor, ensure new variables get their defaults and removed ones are pruned.
  // Also handles baseline shifts after save (savedDefaultsByName changes).
  const prevCellKeysRef = useRef<string | null>(null)
  const prevSavedDefaultsRef = useRef(savedDefaultsByName)
  const cellKeys = useMemo(() => {
    const allCells = flattenCellsForExecution(cells)
    return allCells
      .filter((c) => c.type === 'variable')
      .map((c) => `${c.name}:${(c as VariableCellConfig).defaultValue ?? ''}`)
      .join('|')
  }, [cells])

  if (prevCellKeysRef.current !== null &&
      (prevCellKeysRef.current !== cellKeys || prevSavedDefaultsRef.current !== savedDefaultsByName)) {
    // Recompute: add new variables with defaults, prune removed ones,
    // but preserve user-set values for existing variables.
    const allCells = flattenCellsForExecution(cells)
    const knownNames = new Set(
      allCells.filter((c) => c.type === 'variable').map((c) => c.name)
    )
    const next: Record<string, VariableValue> = {}
    for (const cell of allCells) {
      if (cell.type === 'variable') {
        const varCell = cell as VariableCellConfig
        const existing = variableValuesRef.current[cell.name]
        if (existing !== undefined) {
          next[cell.name] = existing
        } else {
          const savedDefault = savedDefaultsByName.get(cell.name)
          const baseline = savedDefault ?? varCell.defaultValue
          if (baseline !== undefined) {
            next[cell.name] = baseline
          } else if (varCell.variableType === 'text') {
            next[cell.name] = ''
          }
        }
      }
    }
    // Prune variables that no longer have cells
    for (const name of Object.keys(variableValuesRef.current)) {
      if (!knownNames.has(name)) {
        delete next[name]
      }
    }
    variableValuesRef.current = next
    setVariableValues(next)
  }
  prevCellKeysRef.current = cellKeys
  prevSavedDefaultsRef.current = savedDefaultsByName

  // Sync local state → URL for bookmarkability (complete snapshot on each change).
  // Writes all variable params at once, avoiding stale-closure issues with
  // incremental setSearchParams calls during rapid updates.
  // Also cleans up URL params for variables removed by cell-sync.
  useEffect(() => {
    const allCells = flattenCellsForExecution(cells)
    setSearchParams(prev => {
      const next = new URLSearchParams(prev)
      // Clear all non-reserved params (variable params)
      const keysToDelete: string[] = []
      next.forEach((_, key) => {
        if (!RESERVED_URL_PARAMS.has(key)) {
          keysToDelete.push(key)
        }
      })
      keysToDelete.forEach(key => next.delete(key))
      // Write deltas from saved defaults
      for (const [name, value] of Object.entries(variableValues)) {
        const savedDefault = savedDefaultsByName.get(name)
        const currentCell = allCells.find(c => c.type === 'variable' && c.name === name) as
          | VariableCellConfig
          | undefined
        const baseline = savedDefault ?? currentCell?.defaultValue
        if (baseline === undefined || !variableValuesEqual(value, baseline)) {
          next.set(name, serializeVariableValue(value))
        }
      }
      return next
    }, { replace: true })
  }, [variableValues, cells, savedDefaultsByName, setSearchParams])

  // Set a variable value: updates local state + ref (URL synced by effect above)
  const setVariableValue = useCallback(
    (cellName: string, value: VariableValue) => {
      // Update ref immediately for synchronous access during execution
      variableValuesRef.current = { ...variableValuesRef.current, [cellName]: value }
      // Update React state for re-render
      setVariableValues(prev => ({ ...prev, [cellName]: value }))
    },
    []
  )

  // Migrate variable from old name to new name when cell is renamed
  const migrateVariable = useCallback(
    (oldName: string, newName: string) => {
      const oldValue = variableValuesRef.current[oldName]
      if (oldValue !== undefined) {
        const nextValues = { ...variableValuesRef.current }
        nextValues[newName] = oldValue
        delete nextValues[oldName]
        variableValuesRef.current = nextValues
        setVariableValues(nextValues)
      }
    },
    []
  )

  // Remove variable when cell is deleted
  const removeVariable = useCallback(
    (cellName: string) => {
      const nextValues = { ...variableValuesRef.current }
      delete nextValues[cellName]
      variableValuesRef.current = nextValues
      setVariableValues(nextValues)
    },
    []
  )

  return {
    variableValues,
    variableValuesRef,
    setVariableValue,
    migrateVariable,
    removeVariable,
  }
}
