/**
 * Perfetto trace generation controller for the Performance Analysis page.
 *
 * Owns the self-contained trace state (generation progress, errors, and the
 * cached buffer reused across open/download) plus the open/download handlers.
 * Extracted from PerformanceAnalysisPage.tsx (#1089); behavior is unchanged —
 * the page renders the SplitButton and the trace banners from this hook.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchPerfettoTrace, triggerTraceDownload } from '@/lib/perfetto-trace'
import { openInPerfetto, PerfettoError } from '@/lib/perfetto'

interface TimeRangeParsed {
  from: Date
  to: Date
}

interface UsePerfettoTraceParams {
  processId: string
  timeRangeParsed: TimeRangeParsed
  dataSource?: string
}

export interface UsePerfettoTraceResult {
  isGenerating: boolean
  traceMode: 'perfetto' | 'download' | null
  progress: { type: 'progress'; message: string } | null
  traceError: string | null
  cachedTraceBuffer: ArrayBuffer | null
  handleOpenInPerfetto: () => Promise<void>
  handleDownloadTrace: () => Promise<void>
  downloadCachedBuffer: () => void
  dismissTraceError: () => void
}

export function usePerfettoTrace({
  processId,
  timeRangeParsed,
  dataSource,
}: UsePerfettoTraceParams): UsePerfettoTraceResult {
  const traceAbortRef = useRef<AbortController | null>(null)
  const [isGenerating, setIsGenerating] = useState(false)
  const [traceMode, setTraceMode] = useState<'perfetto' | 'download' | null>(null)
  const [progress, setProgress] = useState<{ type: 'progress'; message: string } | null>(null)
  const [traceError, setTraceError] = useState<string | null>(null)
  const [cachedTraceBuffer, setCachedTraceBuffer] = useState<ArrayBuffer | null>(null)
  const [cachedTraceTimeRange, setCachedTraceTimeRange] = useState<{ begin: string; end: string } | null>(null)

  // Abort in-flight trace fetch on unmount
  useEffect(() => {
    return () => traceAbortRef.current?.abort()
  }, [])

  const canUseCachedBuffer = useCallback(() => {
    if (!cachedTraceBuffer || !cachedTraceTimeRange) return false
    const currentBegin = timeRangeParsed.from.toISOString()
    const currentEnd = timeRangeParsed.to.toISOString()
    return cachedTraceTimeRange.begin === currentBegin && cachedTraceTimeRange.end === currentEnd
  }, [cachedTraceBuffer, cachedTraceTimeRange, timeRangeParsed])

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

    triggerTraceDownload(cachedTraceBuffer, processId)
    setTraceError(null)
  }, [processId, cachedTraceBuffer])

  const handleOpenInPerfetto = useCallback(async () => {
    if (!processId) return

    if (canUseCachedBuffer()) {
      await openCachedInPerfetto()
      return
    }

    traceAbortRef.current?.abort()
    traceAbortRef.current = new AbortController()

    setIsGenerating(true)
    setTraceMode('perfetto')
    setProgress(null)
    setTraceError(null)
    setCachedTraceBuffer(null)
    setCachedTraceTimeRange(null)

    const currentTimeRange = {
      begin: timeRangeParsed.from.toISOString(),
      end: timeRangeParsed.to.toISOString(),
    }

    try {
      const buffer = await fetchPerfettoTrace({
        processId,
        spanType: 'both',
        timeRange: currentTimeRange,
        onProgress: (message) => setProgress({ type: 'progress', message }),
        signal: traceAbortRef.current.signal,
        dataSource,
      })

      setCachedTraceBuffer(buffer)
      setCachedTraceTimeRange(currentTimeRange)

      await openInPerfetto({
        buffer,
        processId,
        timeRange: currentTimeRange,
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
  }, [processId, canUseCachedBuffer, openCachedInPerfetto, timeRangeParsed, dataSource])

  const handleDownloadTrace = useCallback(async () => {
    if (!processId) return

    // If we have cached buffer, download it directly
    if (canUseCachedBuffer()) {
      downloadCachedBuffer()
      return
    }

    traceAbortRef.current?.abort()
    traceAbortRef.current = new AbortController()

    setIsGenerating(true)
    setTraceMode('download')
    setProgress(null)
    setTraceError(null)
    setCachedTraceBuffer(null)
    setCachedTraceTimeRange(null)

    const currentTimeRange = {
      begin: timeRangeParsed.from.toISOString(),
      end: timeRangeParsed.to.toISOString(),
    }

    try {
      const buffer = await fetchPerfettoTrace({
        processId,
        spanType: 'both',
        timeRange: currentTimeRange,
        onProgress: (message) => setProgress({ type: 'progress', message }),
        signal: traceAbortRef.current.signal,
        dataSource,
      })

      setCachedTraceBuffer(buffer)
      setCachedTraceTimeRange(currentTimeRange)

      triggerTraceDownload(buffer, processId)
    } catch (error) {
      const message = error instanceof Error ? error.message : 'Unknown error occurred'
      setTraceError(message)
    } finally {
      setIsGenerating(false)
      setTraceMode(null)
      setProgress(null)
    }
  }, [processId, canUseCachedBuffer, downloadCachedBuffer, timeRangeParsed, dataSource])

  const dismissTraceError = useCallback(() => setTraceError(null), [])

  return {
    isGenerating,
    traceMode,
    progress,
    traceError,
    cachedTraceBuffer,
    handleOpenInPerfetto,
    handleDownloadTrace,
    downloadCachedBuffer,
    dismissTraceError,
  }
}
