import { useState, useCallback, useEffect } from 'react'
import { AlertTriangle, Download, ExternalLink } from 'lucide-react'
import { SplitButton } from '@/components/ui/SplitButton'
import { generateTrace } from '@/lib/api'
import { openInPerfetto, PerfettoError } from '@/lib/perfetto'
import type { CellTypeMetadata, CellRendererProps, CellEditorProps } from '../cell-registry'
import type { PerfettoExportCellConfig, CellConfig, CellState } from '../notebook-types'
import { getVariableString } from '../notebook-types'
import type { GenerateTraceRequest, ProgressUpdate } from '@/types'

// =============================================================================
// Helpers
// =============================================================================

function triggerDownload(buffer: ArrayBuffer, processId: string): void {
  const blob = new Blob([buffer], { type: 'application/octet-stream' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `trace-${processId}.pb`
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}

// =============================================================================
// Renderer Component
// =============================================================================

export function PerfettoExportCell({
  options,
  timeRange,
  variables,
}: CellRendererProps) {
  // Generation state
  const [isGenerating, setIsGenerating] = useState(false)
  const [traceMode, setTraceMode] = useState<'perfetto' | 'download' | null>(null)
  const [progress, setProgress] = useState<ProgressUpdate | null>(null)
  const [traceError, setTraceError] = useState<string | null>(null)

  // Cache to avoid regenerating on repeated clicks
  const [cachedTraceBuffer, setCachedTraceBuffer] = useState<ArrayBuffer | null>(null)
  const [cachedTraceTimeRange, setCachedTraceTimeRange] = useState<{ begin: string; end: string } | null>(null)

  const processIdVar = (options?.processIdVar as string) ?? '$process_id'
  const spanType = (options?.spanType as 'thread' | 'async' | 'both') ?? 'both'

  // Strip $ prefix to get variable name for lookup
  const varName = processIdVar.startsWith('$') ? processIdVar.slice(1) : processIdVar
  const processIdValue = variables[varName]
  const processId = processIdValue !== undefined ? getVariableString(processIdValue) : ''
  const hasProcessId = processId !== ''

  // Clear cache when processId or spanType changes
  useEffect(() => {
    setCachedTraceBuffer(null)
    setCachedTraceTimeRange(null)
  }, [processId, spanType])

  // Cache validation
  const canUseCachedBuffer = useCallback(() => {
    if (!cachedTraceBuffer || !cachedTraceTimeRange) return false
    return cachedTraceTimeRange.begin === timeRange.begin &&
           cachedTraceTimeRange.end === timeRange.end
  }, [cachedTraceBuffer, cachedTraceTimeRange, timeRange])

  // Build trace request based on spanType
  const buildTraceRequest = useCallback((): GenerateTraceRequest => {
    return {
      include_thread_spans: spanType !== 'async',
      include_async_spans: spanType !== 'thread',
      time_range: timeRange,
    }
  }, [spanType, timeRange])

  const openCachedInPerfetto = useCallback(async () => {
    if (!processId || !cachedTraceBuffer || !cachedTraceTimeRange) return

    setIsGenerating(true)
    setTraceMode('perfetto')
    setTraceError(null)

    try {
      await openInPerfetto({
        buffer: cachedTraceBuffer,
        processId,
        timeRange: cachedTraceTimeRange,
        onProgress: (message) => setProgress({ type: 'progress', message }),
      })
    } catch (error) {
      const perfettoError = error as PerfettoError
      setTraceError(perfettoError.message || 'Unknown error occurred')
    } finally {
      setIsGenerating(false)
      setTraceMode(null)
      setProgress(null)
    }
  }, [processId, cachedTraceBuffer, cachedTraceTimeRange])

  const downloadCachedBuffer = useCallback(() => {
    if (!processId || !cachedTraceBuffer) return
    triggerDownload(cachedTraceBuffer, processId)
    setTraceError(null)
  }, [processId, cachedTraceBuffer])

  const handleOpenInPerfetto = async () => {
    if (!processId) return

    if (canUseCachedBuffer()) {
      await openCachedInPerfetto()
      return
    }

    setIsGenerating(true)
    setTraceMode('perfetto')
    setProgress(null)
    setTraceError(null)
    setCachedTraceBuffer(null)
    setCachedTraceTimeRange(null)

    try {
      const buffer = await generateTrace(processId, buildTraceRequest(), (update) => {
        setProgress(update)
      }, { returnBuffer: true })

      if (!buffer) {
        throw new Error('No trace data received')
      }

      setCachedTraceBuffer(buffer)
      setCachedTraceTimeRange(timeRange)

      await openInPerfetto({
        buffer,
        processId,
        timeRange,
        onProgress: (message) => setProgress({ type: 'progress', message }),
      })
    } catch (error) {
      const perfettoError = error as PerfettoError
      if (perfettoError.type) {
        setTraceError(perfettoError.message)
      } else {
        const message = error instanceof Error ? error.message : 'Unknown error occurred'
        setTraceError(message)
      }
    } finally {
      setIsGenerating(false)
      setTraceMode(null)
      setProgress(null)
    }
  }

  const handleDownloadTrace = async () => {
    if (!processId) return

    // If we have cached buffer, download it directly
    if (canUseCachedBuffer()) {
      downloadCachedBuffer()
      return
    }

    setIsGenerating(true)
    setTraceMode('download')
    setProgress(null)
    setTraceError(null)
    setCachedTraceBuffer(null)
    setCachedTraceTimeRange(null)

    try {
      const buffer = await generateTrace(processId, buildTraceRequest(), (update) => {
        setProgress(update)
      }, { returnBuffer: true })

      if (!buffer) {
        throw new Error('No trace data received')
      }

      // Cache for potential subsequent "Open in Perfetto"
      setCachedTraceBuffer(buffer)
      setCachedTraceTimeRange(timeRange)

      triggerDownload(buffer, processId)
    } catch (error) {
      const message = error instanceof Error ? error.message : 'Unknown error occurred'
      setTraceError(message)
    } finally {
      setIsGenerating(false)
      setTraceMode(null)
      setProgress(null)
    }
  }

  return (
    <div className="flex flex-col gap-2 py-2">
      {/* Warning if variable not found */}
      {!hasProcessId && (
        <div className="flex items-center gap-2 px-3 py-2 bg-amber-500/10 border border-amber-500/30 rounded-md">
          <AlertTriangle className="w-4 h-4 text-amber-500" />
          <span className="text-sm text-amber-500">
            Variable "{processIdVar}" not found. Add a Variable cell above.
          </span>
        </div>
      )}

      {/* Error state with retry */}
      {traceError && (
        <div className="flex items-start gap-2 px-3 py-2 bg-red-500/10 border border-red-500/30 rounded-md">
          <AlertTriangle className="w-4 h-4 text-red-500 mt-0.5" />
          <div className="flex-1">
            <span className="text-sm text-red-400">{traceError}</span>
            <div className="flex gap-2 mt-2">
              <button
                onClick={() => setTraceError(null)}
                className="px-2 py-1 text-xs bg-app-panel border border-theme-border rounded text-theme-text-primary hover:bg-app-bg"
              >
                Dismiss
              </button>
              <button
                onClick={handleOpenInPerfetto}
                className="px-2 py-1 text-xs bg-accent-link text-white rounded hover:bg-accent-link/90"
              >
                Retry
              </button>
              {cachedTraceBuffer && (
                <button
                  onClick={downloadCachedBuffer}
                  className="px-2 py-1 text-xs bg-app-panel border border-theme-border rounded text-theme-text-primary hover:bg-app-bg flex items-center gap-1"
                >
                  <Download className="w-3 h-3" />
                  Download Instead
                </button>
              )}
            </div>
          </div>
        </div>
      )}

      {/* Action buttons and status on same line */}
      <div className="flex items-center gap-3">
        <SplitButton
          primaryLabel="Open in Perfetto"
          primaryIcon={<ExternalLink className="w-4 h-4" />}
          onPrimaryClick={handleOpenInPerfetto}
          secondaryActions={[
            {
              label: 'Download',
              icon: <Download className="w-4 h-4" />,
              onClick: handleDownloadTrace,
            },
          ]}
          disabled={isGenerating || !hasProcessId}
          loading={isGenerating}
          loadingLabel={traceMode === 'perfetto' ? 'Opening...' : 'Downloading...'}
        />

        {/* Progress indicator inline */}
        {isGenerating && progress && (
          <span className="text-xs text-theme-text-secondary truncate">
            {progress.message}
          </span>
        )}

        {/* Span type info when not generating */}
        {!isGenerating && hasProcessId && (
          <span className="text-xs text-theme-text-muted">
            {spanType === 'both' ? 'Thread + Async spans' : spanType === 'thread' ? 'Thread spans only' : 'Async spans only'}
          </span>
        )}
      </div>
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function PerfettoExportCellEditor({ config, onChange, variables }: CellEditorProps) {
  const perfConfig = config as PerfettoExportCellConfig

  // Get list of available variable names for reference
  const availableVars = Object.keys(variables)

  // Validation
  const varName = (perfConfig.processIdVar || '$process_id').replace(/^\$/, '')
  const varExists = varName in variables

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Process ID Variable
        </label>
        <input
          type="text"
          value={perfConfig.processIdVar || '$process_id'}
          onChange={(e) => onChange({ ...perfConfig, processIdVar: e.target.value })}
          className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
          placeholder="$process_id"
        />
        <p className="text-xs text-theme-text-muted mt-1">
          Name of the variable containing the process ID
        </p>
        {availableVars.length > 0 && (
          <p className="text-xs text-theme-text-muted mt-1">
            Available: {availableVars.map(v => `$${v}`).join(', ')}
          </p>
        )}
        {!varExists && perfConfig.processIdVar && (
          <div className="text-red-400 text-sm mt-1">
            Variable "{perfConfig.processIdVar}" not found
          </div>
        )}
      </div>

      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Span Type
        </label>
        <select
          value={perfConfig.spanType || 'both'}
          onChange={(e) =>
            onChange({ ...perfConfig, spanType: e.target.value as 'thread' | 'async' | 'both' })
          }
          className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
        >
          <option value="both">Both (Thread + Async)</option>
          <option value="thread">Thread Spans Only</option>
          <option value="async">Async Spans Only</option>
        </select>
        <p className="text-xs text-theme-text-muted mt-1">
          Which span types to include in the trace
        </p>
      </div>
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

// eslint-disable-next-line react-refresh/only-export-components
export const perfettoExportMetadata: CellTypeMetadata = {
  renderer: PerfettoExportCell,
  EditorComponent: PerfettoExportCellEditor,

  label: 'Perfetto Export',
  icon: 'E',
  description: 'Export spans to Perfetto trace viewer',
  showTypeBadge: true,
  defaultHeight: 80,

  canBlockDownstream: false,  // No data output for other cells

  createDefaultConfig: () => ({
    type: 'perfettoexport' as const,
    processIdVar: '$process_id',
    spanType: 'both',
  }),

  // No execute method - action is user-triggered via button

  getRendererProps: (config: CellConfig, state: CellState) => {
    const perfConfig = config as PerfettoExportCellConfig
    return {
      status: state.status,
      options: {
        processIdVar: perfConfig.processIdVar ?? '$process_id',
        spanType: perfConfig.spanType ?? 'both',
      },
    }
  },
}
