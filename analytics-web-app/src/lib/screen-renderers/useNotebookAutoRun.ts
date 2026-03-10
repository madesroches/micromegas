import { useCallback, useEffect, useRef } from 'react'

interface UseNotebookAutoRunParams {
  executeFromCellByName: (name: string) => Promise<void>
  /** Execute cells after the named cell (skips the cell itself) */
  executeAfterCellByName: (name: string) => Promise<void>
}

interface UseNotebookAutoRunResult {
  /** Schedule a debounced auto-run: re-executes the cell and all cells below (for config editing) */
  scheduleAutoRun: (cellName: string) => void
  /** Trigger immediate auto-run: executes only cells *after* the named cell (for value selection) */
  triggerAutoRun: (cellName: string, autoRunFromHere?: boolean) => void
}

/**
 * Manages auto-run behavior for notebook cells.
 *
 * Two distinct auto-run triggers:
 * - **Config editing** (scheduleAutoRun): When a cell's configuration changes
 *   (e.g. SQL editing), the cell itself is re-executed along with all cells below
 *   it, after a 300ms debounce.
 * - **Value selection** (triggerAutoRun): When a user selects a value in a
 *   variable cell (e.g. dropdown), only the cells *after* the variable are
 *   executed — the variable cell's own query is not re-run since the value is
 *   already set by the user interaction.
 *
 * A re-entrance guard prevents recursive auto-run when execution itself sets
 * variables (e.g. expression variables computed during execution).
 */
export function useNotebookAutoRun({
  executeFromCellByName,
  executeAfterCellByName,
}: UseNotebookAutoRunParams): UseNotebookAutoRunResult {
  // Guard ref prevents re-entrance when auto-run itself sets variables
  const autoRunningRef = useRef(false)

  // Ref to always access the latest executeFromCellByName inside debounced timers
  const executeFromCellByNameRef = useRef(executeFromCellByName)
  executeFromCellByNameRef.current = executeFromCellByName

  // Ref for executeAfterCellByName (used by triggerAutoRun for value changes)
  const executeAfterCellByNameRef = useRef(executeAfterCellByName)
  executeAfterCellByNameRef.current = executeAfterCellByName

  // Debounced auto-run timers (per cell name) for config changes like SQL editing
  const autoRunTimersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map())
  useEffect(() => {
    const timers = autoRunTimersRef.current
    return () => {
      for (const timer of timers.values()) clearTimeout(timer)
      timers.clear()
    }
  }, [])

  const scheduleAutoRun = useCallback(
    (cellName: string) => {
      const timers = autoRunTimersRef.current
      const existing = timers.get(cellName)
      if (existing) clearTimeout(existing)
      timers.set(cellName, setTimeout(() => {
        timers.delete(cellName)
        executeFromCellByNameRef.current(cellName)
      }, 300))
    },
    [],
  )

  const triggerAutoRun = useCallback(
    (cellName: string, autoRunFromHere?: boolean) => {
      if (!autoRunFromHere || autoRunningRef.current) return
      autoRunningRef.current = true
      // Skip re-executing the variable cell itself — its value was already set
      // by the user interaction; only execute downstream cells.
      executeAfterCellByNameRef.current(cellName).finally(() => {
        autoRunningRef.current = false
      })
    },
    [],
  )

  return { scheduleAutoRun, triggerAutoRun }
}
