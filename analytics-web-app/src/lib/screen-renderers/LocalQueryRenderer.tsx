import { useState, useCallback, useEffect, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { Table, tableFromIPC } from 'apache-arrow'
import { registerRenderer, type ScreenRendererProps } from './index'
import { useDefaultSaveCleanup, useExposeSaveRef } from '../url-cleanup-utils'
import { useTimeRangeSync } from './useTimeRangeSync'
import { LoadingState, EmptyState } from './shared'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { DataSourceField } from '@/components/DataSourceSelector'
import { TableBody, type TableColumn } from './table-utils'
import { fetchQueryIPC } from '../arrow-stream'
import { loadWasmEngine } from '../wasm-engine'

interface LocalQueryConfig {
  timeRangeFrom?: string
  timeRangeTo?: string
  dataSource?: string
  sourceSql: string
  sourceTableName: string
  localSql: string
  [key: string]: unknown
}

type WasmQueryEngine = {
  register_table(name: string, ipc_bytes: Uint8Array): number
  execute_sql(sql: string): Promise<Uint8Array>
  reset(): void
}

export function LocalQueryRenderer({
  config,
  onConfigChange,
  timeRange,
  rawTimeRange,
  timeRangeLabel,
  onSave,
  onSaveRef,
  dataSource,
}: ScreenRendererProps) {
  const localConfig = config as unknown as LocalQueryConfig
  const effectiveDataSource = localConfig.dataSource || dataSource

  const [, setSearchParams] = useSearchParams()
  const handleSave = useDefaultSaveCleanup(onSave, setSearchParams)
  useExposeSaveRef(onSaveRef, handleSave)
  useTimeRangeSync({ rawTimeRange, config, onConfigChange })

  // WASM engine
  const [engine, setEngine] = useState<WasmQueryEngine | null>(null)
  const [wasmError, setWasmError] = useState<string | null>(null)
  useEffect(() => {
    loadWasmEngine()
      .then((mod) => setEngine(new mod.WasmQueryEngine()))
      .catch((e) => setWasmError(`Failed to load WASM engine: ${e.message}`))
  }, [])

  // Source query state
  const [sourceStatus, setSourceStatus] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle')
  const [sourceRowCount, setSourceRowCount] = useState(0)
  const [sourceByteSize, setSourceByteSize] = useState(0)
  const [sourceError, setSourceError] = useState<string | null>(null)

  // Local query state
  const [localResult, setLocalResult] = useState<Table | null>(null)
  const [localStatus, setLocalStatus] = useState<'idle' | 'loading' | 'done' | 'error'>('idle')
  const [localError, setLocalError] = useState<string | null>(null)

  // Abort controller for source fetches
  const abortRef = useRef<AbortController | null>(null)
  useEffect(() => () => abortRef.current?.abort(), [])

  // Fetch source data → register in WASM
  const fetchAndRegister = useCallback(async () => {
    if (!engine) return
    abortRef.current?.abort()
    const controller = new AbortController()
    abortRef.current = controller
    setSourceStatus('loading')
    setSourceError(null)
    try {
      const ipcBytes = await fetchQueryIPC(
        {
          sql: localConfig.sourceSql,
          begin: timeRange.begin,
          end: timeRange.end,
          dataSource: effectiveDataSource,
        },
        controller.signal,
      )
      engine.reset()
      const rowCount = engine.register_table(localConfig.sourceTableName, ipcBytes)
      setSourceRowCount(rowCount)
      setSourceByteSize(ipcBytes.byteLength)
      setSourceStatus('ready')
    } catch (e: unknown) {
      if (!controller.signal.aborted) {
        setSourceError(e instanceof Error ? e.message : String(e))
        setSourceStatus('error')
      }
    }
  }, [engine, localConfig.sourceSql, localConfig.sourceTableName, timeRange, effectiveDataSource])

  // Execute local query against WASM
  const executeLocal = useCallback(async () => {
    if (!engine) return
    setLocalStatus('loading')
    setLocalError(null)
    try {
      const ipcBytes = await engine.execute_sql(localConfig.localSql)
      const table = tableFromIPC(ipcBytes)
      setLocalResult(table)
      setLocalStatus('done')
    } catch (e: unknown) {
      setLocalError(e instanceof Error ? e.message : String(e))
      setLocalStatus('error')
    }
  }, [engine, localConfig.localSql])

  // Config change handlers
  const handleSourceSqlChange = useCallback((sql: string) => {
    onConfigChange((prev) => ({ ...prev, sourceSql: sql }))
  }, [onConfigChange])

  const handleTableNameChange = useCallback((name: string) => {
    onConfigChange((prev) => ({ ...prev, sourceTableName: name }))
  }, [onConfigChange])

  const handleLocalSqlChange = useCallback((sql: string) => {
    onConfigChange((prev) => ({ ...prev, localSql: sql }))
  }, [onConfigChange])

  const handleDataSourceChange = useCallback((ds: string) => {
    onConfigChange((prev) => ({ ...prev, dataSource: ds }))
  }, [onConfigChange])

  // Format byte size
  const formatBytes = (bytes: number): string => {
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  }

  // Render results table
  const renderResultTable = () => {
    if (localStatus === 'loading') {
      return <LoadingState message="Executing local query..." />
    }

    if (localResult && localResult.numRows > 0) {
      const columns: TableColumn[] = localResult.schema.fields.map((field) => ({
        name: field.name,
        type: field.type,
      }))

      return (
        <div className="flex-1 overflow-auto bg-app-panel border border-theme-border rounded-lg">
          <table className="w-full">
            <thead className="sticky top-0">
              <tr className="bg-app-card border-b border-theme-border">
                {columns.map((col) => (
                  <th
                    key={col.name}
                    className="px-3 py-2 text-left text-xs font-medium text-theme-text-muted uppercase tracking-wider"
                  >
                    {col.name}
                  </th>
                ))}
              </tr>
            </thead>
            <TableBody data={localResult} columns={columns} />
          </table>
        </div>
      )
    }

    if (localStatus === 'done') {
      return <EmptyState message="Query returned no results." />
    }

    return null
  }

  if (wasmError) {
    return (
      <div className="flex-1 flex items-center justify-center p-6">
        <div className="p-4 bg-accent-error/10 border border-accent-error/50 rounded-lg max-w-lg">
          <p className="text-sm text-accent-error">{wasmError}</p>
        </div>
      </div>
    )
  }

  return (
    <div className="flex h-full">
      <div className="flex-1 flex flex-col p-6 min-w-0 gap-4 overflow-auto">
        {/* Source Query Section */}
        <section className="bg-app-panel border border-theme-border rounded-lg p-4">
          <div className="flex items-center justify-between mb-3">
            <h3 className="text-sm font-semibold text-theme-text-primary">Source Query</h3>
            <div className="flex items-center gap-3 text-xs text-theme-text-muted">
              {sourceStatus === 'ready' && (
                <span>
                  {sourceRowCount.toLocaleString()} rows ({formatBytes(sourceByteSize)})
                </span>
              )}
              {sourceStatus === 'loading' && (
                <span className="flex items-center gap-1.5">
                  <span className="animate-spin rounded-full h-3 w-3 border border-accent-link border-t-transparent" />
                  Fetching...
                </span>
              )}
            </div>
          </div>

          <DataSourceField
            value={effectiveDataSource || ''}
            onChange={handleDataSourceChange}
            className="mb-3"
          />

          <div className="flex items-center gap-2 mb-3">
            <label className="text-xs text-theme-text-muted">Table name:</label>
            <input
              type="text"
              value={localConfig.sourceTableName || 'data'}
              onChange={(e) => handleTableNameChange(e.target.value)}
              className="px-2 py-1 text-xs bg-app-card border border-theme-border rounded text-theme-text-primary focus:outline-none focus:border-accent-link"
            />
          </div>

          <SyntaxEditor
            value={localConfig.sourceSql || ''}
            onChange={handleSourceSqlChange}
            language="sql"
            minHeight="120px"
          />

          {sourceError && (
            <div className="mt-2 p-2 bg-accent-error/10 border border-accent-error/50 rounded-md">
              <p className="text-xs text-accent-error">{sourceError}</p>
            </div>
          )}

          <div className="mt-3 flex items-center gap-3">
            <button
              onClick={fetchAndRegister}
              disabled={!engine || sourceStatus === 'loading'}
              className="px-3 py-1.5 text-xs font-medium rounded bg-accent-link text-white hover:bg-accent-link/90 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Fetch & Register
            </button>
            {timeRangeLabel && (
              <span className="text-xs text-theme-text-muted">
                Time range: {timeRangeLabel}
              </span>
            )}
          </div>
        </section>

        {/* Local Query Section */}
        <section className="bg-app-panel border border-theme-border rounded-lg p-4">
          <div className="flex items-center justify-between mb-3">
            <h3 className="text-sm font-semibold text-theme-text-primary">Local Query</h3>
            <div className="flex items-center gap-3 text-xs text-theme-text-muted">
              {localStatus === 'done' && localResult && (
                <span>{localResult.numRows.toLocaleString()} rows</span>
              )}
            </div>
          </div>

          <SyntaxEditor
            value={localConfig.localSql || ''}
            onChange={handleLocalSqlChange}
            language="sql"
            minHeight="80px"
          />

          {localError && (
            <div className="mt-2 p-2 bg-accent-error/10 border border-accent-error/50 rounded-md">
              <p className="text-xs text-accent-error">{localError}</p>
            </div>
          )}

          <div className="mt-3">
            <button
              onClick={executeLocal}
              disabled={!engine || sourceStatus !== 'ready' || localStatus === 'loading'}
              className="px-3 py-1.5 text-xs font-medium rounded bg-accent-link text-white hover:bg-accent-link/90 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Run
            </button>
            {sourceStatus !== 'ready' && sourceStatus !== 'loading' && (
              <span className="ml-3 text-xs text-theme-text-muted">
                Fetch source data first
              </span>
            )}
          </div>
        </section>

        {/* Results */}
        {renderResultTable()}
      </div>
    </div>
  )
}

registerRenderer('local_query', LocalQueryRenderer)
