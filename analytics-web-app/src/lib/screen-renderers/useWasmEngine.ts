import { useState, useEffect } from 'react'
import { NotebookQueryEngine } from './useCellExecution'
import { loadWasmEngine } from '@/lib/wasm-engine'

interface UseWasmEngineResult {
  engine: NotebookQueryEngine | null
  engineError: string | null
}

/**
 * Loads the WASM query engine asynchronously.
 *
 * The engine is loaded eagerly so remote cell results are always registered
 * for cross-cell references in notebook-local queries.
 */
export function useWasmEngine(): UseWasmEngineResult {
  const [engine, setEngine] = useState<NotebookQueryEngine | null>(null)
  const [engineError, setEngineError] = useState<string | null>(null)

  useEffect(() => {
    if (engine) return
    let cancelled = false
    loadWasmEngine()
      .then((mod) => {
        if (!cancelled) {
          setEngine(new mod.WasmQueryEngine() as unknown as NotebookQueryEngine)
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setEngineError(err instanceof Error ? err.message : 'Failed to load WASM engine')
        }
      })
    return () => { cancelled = true }
  }, [engine])

  return { engine, engineError }
}
