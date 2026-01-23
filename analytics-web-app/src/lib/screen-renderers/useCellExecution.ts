import { useState, useCallback, useEffect, useRef } from 'react'
import { Table } from 'apache-arrow'
import { CellConfig, QueryCellConfig, VariableCellConfig, substituteMacros } from './notebook-utils'
import { CellStatus } from './cell-registry'
import { streamQuery } from '@/lib/arrow-stream'

/** Execution state for a cell */
export interface CellState {
  status: CellStatus
  error?: string
  data: Table | null
  /** For variable cells (combobox): options loaded from query */
  variableOptions?: { label: string; value: string }[]
}

interface UseCellExecutionParams {
  /** Cell configurations from notebook config */
  cells: CellConfig[]
  /** Time range for SQL queries */
  timeRange: { begin: string; end: string }
  /** Ref for synchronous access to variable values during execution */
  variableValuesRef: React.MutableRefObject<Record<string, string>>
  /** Callback to set a variable value (for auto-selecting first option) */
  setVariableValue: (cellName: string, value: string) => void
  /** Refresh trigger from parent (increments to trigger re-execution) */
  refreshTrigger: number
}

export interface UseCellExecutionResult {
  /** Current state of each cell (keyed by cell name) */
  cellStates: Record<string, CellState>
  /** Execute a single cell by index */
  executeCell: (cellIndex: number) => Promise<boolean>
  /** Execute from a cell index through all cells below */
  executeFromCell: (startIndex: number) => Promise<void>
  /** Migrate cell state when a cell is renamed */
  migrateCellState: (oldName: string, newName: string) => void
  /** Remove cell state when a cell is deleted */
  removeCellState: (cellName: string) => void
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
    return new Table()
  }
  return new Table(batches)
}

/**
 * Manages cell execution state for notebooks.
 *
 * Handles:
 * - Tracking loading/success/error states for each cell
 * - Sequential execution with variable substitution
 * - Abort handling for cancelled queries
 * - Auto-execution on mount and refresh
 * - Variable option extraction from query results
 */
export function useCellExecution({
  cells,
  timeRange,
  variableValuesRef,
  setVariableValue,
  refreshTrigger,
}: UseCellExecutionParams): UseCellExecutionResult {
  const [cellStates, setCellStates] = useState<Record<string, CellState>>({})
  const abortControllerRef = useRef<AbortController | null>(null)

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

      // Handle text/number variable cells (no SQL execution)
      if (cell.type === 'variable') {
        const varCell = cell as VariableCellConfig
        if (varCell.variableType === 'text' || varCell.variableType === 'number') {
          setCellStates((prev) => ({
            ...prev,
            [cell.name]: { status: 'success', data: null },
          }))
          return true
        }
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
          // Set default value if not already set
          if (!variableValuesRef.current[cell.name] && options.length > 0) {
            setVariableValue(cell.name, options[0].value)
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
    [cells, timeRange, variableValuesRef, setVariableValue]
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

  // Migrate cell state when a cell is renamed
  const migrateCellState = useCallback((oldName: string, newName: string) => {
    setCellStates((prev) => {
      const next = { ...prev }
      if (oldName in next) {
        next[newName] = next[oldName]
        delete next[oldName]
      }
      return next
    })
  }, [])

  // Remove cell state when a cell is deleted
  const removeCellState = useCallback((cellName: string) => {
    setCellStates((prev) => {
      const next = { ...prev }
      delete next[cellName]
      return next
    })
  }, [])

  return {
    cellStates,
    executeCell,
    executeFromCell,
    migrateCellState,
    removeCellState,
  }
}
