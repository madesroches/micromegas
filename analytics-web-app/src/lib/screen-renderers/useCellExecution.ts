import { useState, useCallback, useEffect, useRef } from 'react'
import { Table } from 'apache-arrow'
import type { CellConfig, CellState, VariableValue } from './notebook-types'
import { getCellTypeMetadata, CellExecutionContext } from './cell-registry'
import { streamQuery } from '@/lib/arrow-stream'

interface UseCellExecutionParams {
  /** Cell configurations from notebook config */
  cells: CellConfig[]
  /** Time range for SQL queries */
  timeRange: { begin: string; end: string }
  /** Ref for synchronous access to variable values during execution */
  variableValuesRef: React.MutableRefObject<Record<string, VariableValue>>
  /** Callback to set a variable value (for auto-selecting first option) */
  setVariableValue: (cellName: string, value: VariableValue) => void
  /** Refresh trigger from parent (increments to trigger re-execution) */
  refreshTrigger: number
  /** Data source name for query routing */
  dataSource?: string
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
  abortSignal: AbortSignal,
  dataSource?: string
): Promise<Table> {
  const batches: import('apache-arrow').RecordBatch[] = []

  for await (const result of streamQuery(
    {
      sql,
      params: { begin: timeRange.begin, end: timeRange.end },
      begin: timeRange.begin,
      end: timeRange.end,
      dataSource,
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
 * - Delegating execution to cell-specific execute methods
 */
export function useCellExecution({
  cells,
  timeRange,
  variableValuesRef,
  setVariableValue,
  refreshTrigger,
  dataSource,
}: UseCellExecutionParams): UseCellExecutionResult {
  const [cellStates, setCellStates] = useState<Record<string, CellState>>({})
  const abortControllerRef = useRef<AbortController | null>(null)

  // Execute a single cell
  const executeCell = useCallback(
    async (cellIndex: number): Promise<boolean> => {
      const cell = cells[cellIndex]
      if (!cell) return false

      const meta = getCellTypeMetadata(cell.type)

      // Cell doesn't have an execute method (e.g., markdown)
      if (!meta.execute) {
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

      // Gather variables from cells above (use ref for synchronous access during execution)
      const availableVariables: Record<string, VariableValue> = {}
      for (let i = 0; i < cellIndex; i++) {
        const prevCell = cells[i]
        if (prevCell.type === 'variable' && variableValuesRef.current[prevCell.name] !== undefined) {
          availableVariables[prevCell.name] = variableValuesRef.current[prevCell.name]
        }
      }

      // Create new abort controller for this execution
      abortControllerRef.current?.abort()
      abortControllerRef.current = new AbortController()

      try {
        // Create execution context - use per-cell data source with fallback to global
        const cellDataSource = ('dataSource' in cell ? cell.dataSource : undefined) || dataSource
        const context: CellExecutionContext = {
          variables: availableVariables,
          timeRange,
          runQuery: (sql) => executeSql(sql, timeRange, abortControllerRef.current!.signal, cellDataSource),
        }

        // Delegate to cell's execute method
        const result = await meta.execute(cell, context)

        // If result is null, nothing was executed (e.g., text variables)
        const newState: CellState = result
          ? { status: 'success', data: result.data ?? null, ...result }
          : { status: 'success', data: null }

        setCellStates((prev) => ({ ...prev, [cell.name]: newState }))

        // Post-execution side effects (e.g., validate/auto-select value for variables)
        if (meta.onExecutionComplete) {
          meta.onExecutionComplete(cell, newState, {
            setVariableValue,
            currentValue: variableValuesRef.current[cell.name],
          })
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
    [cells, timeRange, variableValuesRef, setVariableValue, dataSource]
  )

  // Execute from a cell index (that cell and all below)
  const executeFromCell = useCallback(
    async (startIndex: number) => {
      for (let i = startIndex; i < cells.length; i++) {
        const success = await executeCell(i)
        if (!success) {
          // Mark remaining cells as blocked (except those that don't block)
          for (let j = i + 1; j < cells.length; j++) {
            const blockedCell = cells[j]
            const blockedMeta = getCellTypeMetadata(blockedCell.type)
            if (blockedMeta.canBlockDownstream) {
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

  // Re-execute when time range changes
  const prevTimeRangeRef = useRef(timeRange)
  useEffect(() => {
    if (
      prevTimeRangeRef.current.begin !== timeRange.begin ||
      prevTimeRangeRef.current.end !== timeRange.end
    ) {
      prevTimeRangeRef.current = timeRange
      executeFromCell(0)
    }
  }, [timeRange, executeFromCell])

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
