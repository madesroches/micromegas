import { useState, useCallback, useEffect, useRef } from 'react'
import { Table, tableFromIPC } from 'apache-arrow'
import type { CellConfig, CellState, VariableValue } from './notebook-types'
import { getCellTypeMetadata, CellExecutionContext } from './cell-registry'
import { streamQuery, fetchQueryIPC } from '@/lib/arrow-stream'
import { getTimeRangeForApi } from '@/lib/time-range'
import { resolveCellDataSource } from './notebook-utils'

/** Minimal interface for the WASM query engine (decoupled from WASM module type) */
export interface NotebookQueryEngine {
  register_table(name: string, ipc_bytes: Uint8Array): number
  execute_and_register(sql: string, register_as: string): Promise<Uint8Array>
  deregister_table(name: string): boolean
  reset(): void
}

interface UseCellExecutionParams {
  /** Cell configurations from notebook config */
  cells: CellConfig[]
  /** Raw time range (relative strings like "now-1h") resolved fresh at execution time */
  rawTimeRange: { from: string; to: string }
  /** Ref for synchronous access to variable values during execution */
  variableValuesRef: React.MutableRefObject<Record<string, VariableValue>>
  /** Callback to set a variable value (for auto-selecting first option) */
  setVariableValue: (cellName: string, value: VariableValue) => void
  /** Refresh trigger from parent (increments to trigger re-execution) */
  refreshTrigger: number
  /** Data source name for query routing */
  dataSource?: string
  /** WASM query engine for notebook-local queries (null when not loaded yet) */
  engine: NotebookQueryEngine | null
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
  rawTimeRange,
  variableValuesRef,
  setVariableValue,
  refreshTrigger,
  dataSource,
  engine,
}: UseCellExecutionParams): UseCellExecutionResult {
  const [cellStates, setCellStates] = useState<Record<string, CellState>>({})
  const abortControllerRef = useRef<AbortController | null>(null)
  const cellResultsRef = useRef<Record<string, Table>>({})

  // Helper: update both the ref (for synchronous access) and React state atomically
  const completeCellExecution = useCallback((name: string, state: CellState) => {
    if (state.data.length > 0) {
      cellResultsRef.current = { ...cellResultsRef.current, [name]: state.data[0] }
    }
    setCellStates((prev) => ({ ...prev, [name]: state }))
  }, [])

  // Execute a single cell
  const executeCell = useCallback(
    async (cellIndex: number): Promise<boolean> => {
      const cell = cells[cellIndex]
      if (!cell) return false

      const meta = getCellTypeMetadata(cell.type)

      // Cell doesn't have an execute method (e.g., markdown)
      if (!meta.execute) {
        completeCellExecution(cell.name, { status: 'success', data: [] })
        return true
      }

      // Mark cell as loading (preserve previous data for re-renders during loading)
      setCellStates((prev) => {
        const prevData = prev[cell.name]?.data ?? []
        return {
          ...prev,
          [cell.name]: { ...prev[cell.name], status: 'loading', error: undefined, fetchProgress: undefined, data: prevData },
        }
      })

      const startTime = performance.now()

      // Resolve relative time range to absolute times fresh at execution time
      const timeRange = getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to)

      // Gather variables from cells above (use ref for synchronous access during execution)
      const availableVariables: Record<string, VariableValue> = {}
      const availableCellResults: Record<string, Table> = {}
      for (let i = 0; i < cellIndex; i++) {
        const prevCell = cells[i]
        if (prevCell.type === 'variable' && variableValuesRef.current[prevCell.name] !== undefined) {
          availableVariables[prevCell.name] = variableValuesRef.current[prevCell.name]
        }
        const table = cellResultsRef.current[prevCell.name]
        if (table) availableCellResults[prevCell.name] = table
      }

      // Create new abort controller for this execution
      abortControllerRef.current?.abort()
      abortControllerRef.current = new AbortController()

      try {
        // Create execution context - use per-cell data source with fallback to global
        const cellDataSource = resolveCellDataSource(cell, availableVariables, dataSource)
        const isNotebookSource = cellDataSource === 'notebook'
        const context: CellExecutionContext = {
          variables: availableVariables,
          cellResults: availableCellResults,
          timeRange,
          registerTable: engine
            ? (ipcBytes: Uint8Array) => { engine.register_table(cell.name, ipcBytes) }
            : undefined,
          runQuery: async (sql) => {
            if (isNotebookSource) {
              // Execute locally in WASM engine
              if (!engine) throw new Error('WASM engine not loaded')
              const ipcBytes = await engine.execute_and_register(sql, cell.name)
              return tableFromIPC(ipcBytes)
            } else if (engine) {
              // Remote execution, but register result in WASM for downstream notebook cells
              const ipcBytes = await fetchQueryIPC(
                {
                  sql,
                  params: { begin: timeRange.begin, end: timeRange.end },
                  begin: timeRange.begin,
                  end: timeRange.end,
                  dataSource: cellDataSource,
                },
                abortControllerRef.current!.signal,
                (progress) => {
                  setCellStates((prev) => ({
                    ...prev,
                    [cell.name]: { ...prev[cell.name], fetchProgress: progress },
                  }))
                },
              )
              engine.register_table(cell.name, ipcBytes)
              return tableFromIPC(ipcBytes)
            } else {
              // Remote execution without WASM engine
              return executeSql(sql, timeRange, abortControllerRef.current!.signal, cellDataSource)
            }
          },
          runQueryAs: async (sql, tableName, queryDataSource) => {
            // Resolve per-query data source independently (not from cellDataSource).
            // Variable references ($varname) are substituted, fallback is notebook-level default.
            let resolvedDs = queryDataSource || dataSource
            if (resolvedDs?.startsWith('$')) {
              const varValue = availableVariables[resolvedDs.slice(1)]
              resolvedDs = (typeof varValue === 'string' && varValue) ? varValue : dataSource
            }
            const isLocal = resolvedDs === 'notebook'
            if (isLocal) {
              if (!engine) throw new Error('WASM engine not loaded')
              const ipcBytes = await engine.execute_and_register(sql, tableName)
              return tableFromIPC(ipcBytes)
            } else if (engine) {
              const ipcBytes = await fetchQueryIPC(
                {
                  sql,
                  params: { begin: timeRange.begin, end: timeRange.end },
                  begin: timeRange.begin,
                  end: timeRange.end,
                  dataSource: resolvedDs,
                },
                abortControllerRef.current!.signal,
                (progress) => {
                  setCellStates((prev) => ({
                    ...prev,
                    [cell.name]: { ...prev[cell.name], fetchProgress: progress },
                  }))
                },
              )
              engine.register_table(tableName, ipcBytes)
              return tableFromIPC(ipcBytes)
            } else {
              return executeSql(sql, timeRange, abortControllerRef.current!.signal, resolvedDs)
            }
          },
        }

        // Delegate to cell's execute method
        const result = await meta.execute(cell, context)

        // If result is null, nothing was executed (e.g., text variables)
        const elapsedMs = performance.now() - startTime
        const newState: CellState = result
          ? { status: 'success', data: result.data ?? [], ...result, elapsedMs }
          : { status: 'success', data: [] }

        completeCellExecution(cell.name, newState)

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
        completeCellExecution(cell.name, { status: 'error', error: errorMessage, data: [] })
        return false
      }
    },
    [cells, rawTimeRange, variableValuesRef, setVariableValue, dataSource, engine, completeCellExecution]
  )

  // Execute from a cell index (that cell and all below)
  const executeFromCell = useCallback(
    async (startIndex: number) => {
      // Reset WASM engine when re-executing from the top
      if (startIndex === 0 && engine) {
        engine.reset()
        cellResultsRef.current = {}
      }
      // Reset statuses to idle so useFadeOnIdle detects a change even for
      // fast cells where React would batch loading→success into one render.
      setCellStates((prev) => {
        const next = { ...prev }
        for (let i = startIndex; i < cells.length; i++) {
          const name = cells[i].name
          if (next[name]) {
            next[name] = { ...next[name], status: 'idle' }
          }
        }
        return next
      })
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
                [blockedCell.name]: { status: 'blocked', data: [] },
              }))
            }
          }
          break
        }
      }
    },
    [cells, executeCell, engine]
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
  const prevTimeRangeRef = useRef(rawTimeRange)
  useEffect(() => {
    if (
      prevTimeRangeRef.current.from !== rawTimeRange.from ||
      prevTimeRangeRef.current.to !== rawTimeRange.to
    ) {
      prevTimeRangeRef.current = rawTimeRange
      executeFromCell(0)
    }
  }, [rawTimeRange, executeFromCell])

  // Re-execute all cells when WASM engine becomes available
  // (initial execution may have run before the engine loaded,
  // so remote cell results weren't registered in WASM)
  const prevEngineRef = useRef(engine)
  useEffect(() => {
    if (prevEngineRef.current === null && engine !== null) {
      executeFromCell(0)
    }
    prevEngineRef.current = engine
  }, [engine, executeFromCell])

  // Migrate cell state when a cell is renamed
  const migrateCellState = useCallback((oldName: string, newName: string) => {
    engine?.deregister_table(oldName)
    if (oldName in cellResultsRef.current) {
      const next = { ...cellResultsRef.current }
      next[newName] = next[oldName]
      delete next[oldName]
      cellResultsRef.current = next
    }
    setCellStates((prev) => {
      const next = { ...prev }
      if (oldName in next) {
        next[newName] = next[oldName]
        delete next[oldName]
      }
      return next
    })
  }, [engine])

  // Remove cell state when a cell is deleted
  const removeCellState = useCallback((cellName: string) => {
    engine?.deregister_table(cellName)
    if (cellName in cellResultsRef.current) {
      const next = { ...cellResultsRef.current }
      delete next[cellName]
      cellResultsRef.current = next
    }
    setCellStates((prev) => {
      const next = { ...prev }
      delete next[cellName]
      return next
    })
  }, [engine])

  return {
    cellStates,
    executeCell,
    executeFromCell,
    migrateCellState,
    removeCellState,
  }
}
