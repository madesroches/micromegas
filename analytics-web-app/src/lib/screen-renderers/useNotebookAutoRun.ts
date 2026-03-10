import { useCallback, useEffect, useRef } from 'react'

interface UseNotebookAutoRunParams {
  executeFromCellByName: (name: string) => Promise<void>
  /** Execute cells after the named cell (skips the cell itself) */
  executeAfterCellByName: (name: string) => Promise<void>
}

interface UseNotebookAutoRunResult {
  /** Schedule a debounced auto-run (for config changes like SQL editing) */
  scheduleAutoRun: (cellName: string) => void
  /** Trigger immediate auto-run with re-entrance guard (for variable value changes) */
  triggerAutoRun: (cellName: string, autoRunFromHere?: boolean) => void
}

/**
 * Manages auto-run behavior for notebook cells.
 *
 * Owns:
 * - Re-entrance guard ref preventing recursive auto-run when execution itself sets variables
 * - Debounced per-cell timers for config changes (SQL editing, content changes)
 * - Immediate guarded execution for variable value changes
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
