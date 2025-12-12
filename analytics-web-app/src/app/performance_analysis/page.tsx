'use client'

import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useSearchParams, useRouter, usePathname } from 'next/navigation'
import { useMutation } from '@tanstack/react-query'
import { AppLink } from '@/components/AppLink'
import { SplitButton } from '@/components/ui/SplitButton'
import { AlertCircle, Clock, Download, ExternalLink } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { TimeSeriesChart, ChartAxisBounds } from '@/components/TimeSeriesChart'
import { ThreadCoverageTimeline } from '@/components/ThreadCoverageTimeline'
import { executeSqlQuery, toRowObjects, generateTrace } from '@/lib/api'
import { openInPerfetto, PerfettoError } from '@/lib/perfetto'
import { useTimeRange } from '@/hooks/useTimeRange'
import { GenerateTraceRequest, ProgressUpdate, ThreadSegment, ThreadCoverage } from '@/types'

const DISCOVERY_SQL = `SELECT DISTINCT name, target, unit
FROM view_instance('measures', '$process_id')
ORDER BY name`

const DEFAULT_SQL = `SELECT
  date_bin(INTERVAL '$bin_interval', time) as time,
  max(value) as value
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
GROUP BY date_bin(INTERVAL '$bin_interval', time)
ORDER BY time`

const PROCESS_SQL = `SELECT exe FROM processes WHERE process_id = '$process_id' LIMIT 1`

const THREAD_COVERAGE_SQL = `SELECT
  arrow_cast(stream_id, 'Utf8') as stream_id,
  concat(
    arrow_cast(property_get("streams.properties", 'thread-name'), 'Utf8'),
    '-',
    arrow_cast(property_get("streams.properties", 'thread-id'), 'Utf8')
  ) as thread_name,
  begin_time,
  end_time
FROM blocks
WHERE process_id = '$process_id'
  AND array_has("streams.tags", 'cpu')
ORDER BY stream_id, begin_time`

const TRACE_EVENTS_COUNT_SQL = `SELECT
  SUM(nb_objects) as event_count
FROM blocks
WHERE process_id = '$process_id'
  AND array_has("streams.tags", 'cpu')`

const VARIABLES = [
  { name: 'process_id', description: 'Current process ID' },
  { name: 'measure_name', description: 'Selected measure name' },
  { name: 'bin_interval', description: 'Time bucket size for downsampling' },
]

interface Measure {
  name: string
  target: string
  unit: string
}

function calculateBinInterval(timeSpanMs: number, chartWidthPx: number = 800): string {
  const numBins = chartWidthPx
  const binIntervalMs = timeSpanMs / numBins

  const intervals = [
    { ms: 1, label: '1 millisecond' },
    { ms: 10, label: '10 milliseconds' },
    { ms: 50, label: '50 milliseconds' },
    { ms: 100, label: '100 milliseconds' },
    { ms: 500, label: '500 milliseconds' },
    { ms: 1000, label: '1 second' },
    { ms: 5000, label: '5 seconds' },
    { ms: 10000, label: '10 seconds' },
    { ms: 30000, label: '30 seconds' },
    { ms: 60000, label: '1 minute' },
    { ms: 300000, label: '5 minutes' },
    { ms: 600000, label: '10 minutes' },
    { ms: 1800000, label: '30 minutes' },
    { ms: 3600000, label: '1 hour' },
  ]

  for (const interval of intervals) {
    if (interval.ms >= binIntervalMs) {
      return interval.label
    }
  }
  return '1 hour'
}

function PerformanceAnalysisContent() {
  const searchParams = useSearchParams()
  const router = useRouter()
  const pathname = usePathname()
  const processId = searchParams.get('process_id')
  const measureParam = searchParams.get('measure')
  const { parsed: timeRange, apiTimeRange, setTimeRange } = useTimeRange()

  const [measures, setMeasures] = useState<Measure[]>([])
  const [selectedMeasure, setSelectedMeasure] = useState<string | null>(measureParam)
  const [queryError, setQueryError] = useState<string | null>(null)
  const [chartData, setChartData] = useState<{ time: number; value: number }[]>([])
  const [processExe, setProcessExe] = useState<string | null>(null)
  const [hasLoaded, setHasLoaded] = useState(false)
  const [discoveryDone, setDiscoveryDone] = useState(false)
  const [chartWidth, setChartWidth] = useState<number>(800)
  const [threadCoverage, setThreadCoverage] = useState<ThreadCoverage[]>([])
  const [traceEventCount, setTraceEventCount] = useState<number | null>(null)
  const [traceEventCountLoading, setTraceEventCountLoading] = useState(false)
  const [isGenerating, setIsGenerating] = useState(false)
  const [traceMode, setTraceMode] = useState<'perfetto' | 'download' | null>(null)
  const [progress, setProgress] = useState<ProgressUpdate | null>(null)
  const [traceError, setTraceError] = useState<string | null>(null)
  const [chartAxisBounds, setChartAxisBounds] = useState<ChartAxisBounds | null>(null)
  // Cache the trace buffer so we can retry Perfetto or download without regenerating
  const [cachedTraceBuffer, setCachedTraceBuffer] = useState<ArrayBuffer | null>(null)
  const [cachedTraceTimeRange, setCachedTraceTimeRange] = useState<{ begin: string; end: string } | null>(null)

  const binInterval = useMemo(() => {
    const fromDate = new Date(apiTimeRange.begin)
    const toDate = new Date(apiTimeRange.end)
    const timeSpanMs = toDate.getTime() - fromDate.getTime()
    return calculateBinInterval(timeSpanMs, chartWidth)
  }, [apiTimeRange, chartWidth])

  const selectedMeasureInfo = useMemo(() => {
    return measures.find((m) => m.name === selectedMeasure)
  }, [measures, selectedMeasure])

  // Chart time range for ThreadCoverageTimeline
  const chartTimeRange = useMemo(() => {
    if (chartData.length === 0) return null
    return {
      from: Math.min(...chartData.map((d) => d.time)),
      to: Math.max(...chartData.map((d) => d.time)),
    }
  }, [chartData])

  const discoveryMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      const rows = toRowObjects(data)
      const measureList: Measure[] = rows.map((row) => ({
        name: String(row.name ?? ''),
        target: String(row.target ?? ''),
        unit: String(row.unit ?? ''),
      }))
      setMeasures(measureList)
      setDiscoveryDone(true)

      if (measureList.length > 0 && !selectedMeasure) {
        const deltaTime = measureList.find((m) => m.name === 'DeltaTime')
        setSelectedMeasure(deltaTime ? deltaTime.name : measureList[0].name)
      }
    },
    onError: (err: Error) => {
      setQueryError(err.message)
      setDiscoveryDone(true)
    },
  })

  const dataMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      setQueryError(null)
      const rows = toRowObjects(data)
      const points = rows.map((row) => ({
        time: new Date(String(row.time)).getTime(),
        value: Number(row.value),
      }))
      setChartData(points)
      setHasLoaded(true)
    },
    onError: (err: Error) => {
      setQueryError(err.message)
      setHasLoaded(true)
    },
  })

  const processMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      const rows = toRowObjects(data)
      if (rows.length > 0) {
        setProcessExe(String(rows[0].exe ?? ''))
      }
    },
  })

  const threadCoverageMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      const rows = toRowObjects(data)
      // Group by stream_id (one row per CPU stream/thread)
      const threadMap = new Map<string, ThreadCoverage>()

      for (const row of rows) {
        const streamId = String(row.stream_id ?? '')
        const threadName = String(row.thread_name ?? 'unknown')
        const beginTime = new Date(String(row.begin_time)).getTime()
        const endTime = new Date(String(row.end_time)).getTime()

        if (!threadMap.has(streamId)) {
          threadMap.set(streamId, {
            streamId,
            threadName,
            segments: [],
          })
        }
        threadMap.get(streamId)!.segments.push({ begin: beginTime, end: endTime })
      }

      // Sort threads by name and segments by begin time
      const threads = Array.from(threadMap.values())
      threads.sort((a, b) => a.threadName.localeCompare(b.threadName))
      for (const thread of threads) {
        thread.segments.sort((a, b) => a.begin - b.begin)
      }

      setThreadCoverage(threads)
    },
    onError: (err: Error) => {
      console.error('Failed to fetch thread coverage:', err.message)
      setThreadCoverage([])
    },
  })

  const traceEventCountMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      const rows = toRowObjects(data)
      if (rows.length > 0 && rows[0].event_count != null) {
        setTraceEventCount(Number(rows[0].event_count))
      } else {
        setTraceEventCount(0)
      }
      setTraceEventCountLoading(false)
    },
    onError: (err: Error) => {
      console.error('Failed to fetch trace event count:', err.message)
      setTraceEventCount(0)
      setTraceEventCountLoading(false)
    },
  })

  const discoveryMutateRef = useRef(discoveryMutation.mutate)
  discoveryMutateRef.current = discoveryMutation.mutate
  const dataMutateRef = useRef(dataMutation.mutate)
  dataMutateRef.current = dataMutation.mutate
  const processMutateRef = useRef(processMutation.mutate)
  processMutateRef.current = processMutation.mutate
  const threadCoverageMutateRef = useRef(threadCoverageMutation.mutate)
  threadCoverageMutateRef.current = threadCoverageMutation.mutate
  const traceEventCountMutateRef = useRef(traceEventCountMutation.mutate)
  traceEventCountMutateRef.current = traceEventCountMutation.mutate

  const loadDiscovery = useCallback(() => {
    if (!processId) return
    discoveryMutateRef.current({
      sql: DISCOVERY_SQL,
      params: { process_id: processId },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    })
  }, [processId, apiTimeRange])

  const loadData = useCallback(
    (sql: string = DEFAULT_SQL) => {
      if (!processId || !selectedMeasure) return
      setQueryError(null)
      dataMutateRef.current({
        sql,
        params: {
          process_id: processId,
          measure_name: selectedMeasure,
          bin_interval: binInterval,
        },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    },
    [processId, selectedMeasure, binInterval, apiTimeRange]
  )

  const loadThreadCoverage = useCallback(() => {
    if (!processId) return
    threadCoverageMutateRef.current({
      sql: THREAD_COVERAGE_SQL,
      params: { process_id: processId },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    })
    setTraceEventCountLoading(true)
    traceEventCountMutateRef.current({
      sql: TRACE_EVENTS_COUNT_SQL,
      params: { process_id: processId },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    })
  }, [processId, apiTimeRange])

  const updateMeasure = useCallback(
    (measure: string) => {
      setSelectedMeasure(measure)
      const params = new URLSearchParams(searchParams.toString())
      params.set('measure', measure)
      router.push(`${pathname}?${params.toString()}`)
    },
    [searchParams, router, pathname]
  )

  // Load process info once on mount
  const hasLoadedProcessRef = useRef(false)
  useEffect(() => {
    if (processId && !hasLoadedProcessRef.current) {
      hasLoadedProcessRef.current = true
      processMutateRef.current({
        sql: PROCESS_SQL,
        params: { process_id: processId },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    }
  }, [processId, apiTimeRange])

  // Load measure discovery on mount
  const hasLoadedDiscoveryRef = useRef(false)
  useEffect(() => {
    if (processId && !hasLoadedDiscoveryRef.current) {
      hasLoadedDiscoveryRef.current = true
      loadDiscovery()
      loadThreadCoverage()
    }
  }, [processId, loadDiscovery, loadThreadCoverage])

  // Load data when measure is selected
  useEffect(() => {
    if (discoveryDone && selectedMeasure && processId) {
      loadData()
    }
  }, [discoveryDone, selectedMeasure, processId, loadData])

  // Reload when time range changes
  const prevTimeRangeRef = useRef<{ begin: string; end: string } | null>(null)
  useEffect(() => {
    if (!hasLoaded) return
    if (prevTimeRangeRef.current === null) {
      prevTimeRangeRef.current = { begin: apiTimeRange.begin, end: apiTimeRange.end }
      return
    }
    if (
      prevTimeRangeRef.current.begin !== apiTimeRange.begin ||
      prevTimeRangeRef.current.end !== apiTimeRange.end
    ) {
      prevTimeRangeRef.current = { begin: apiTimeRange.begin, end: apiTimeRange.end }
      hasLoadedDiscoveryRef.current = false
      loadDiscovery()
      loadThreadCoverage()
    }
  }, [apiTimeRange.begin, apiTimeRange.end, hasLoaded, loadDiscovery, loadThreadCoverage])

  const handleRunQuery = useCallback(
    (sql: string) => {
      loadData(sql)
    },
    [loadData]
  )

  const handleResetQuery = useCallback(() => {
    loadData(DEFAULT_SQL)
  }, [loadData])

  const handleRefresh = useCallback(() => {
    hasLoadedDiscoveryRef.current = false
    loadDiscovery()
    loadThreadCoverage()
  }, [loadDiscovery, loadThreadCoverage])

  const handleTimeRangeSelect = useCallback(
    (from: Date, to: Date) => {
      // Zoom into the selected time range
      setTimeRange(from.toISOString(), to.toISOString())
    },
    [setTimeRange]
  )

  const handleChartWidthChange = useCallback((width: number) => {
    setChartWidth(width)
  }, [])

  const handleAxisBoundsChange = useCallback((bounds: ChartAxisBounds) => {
    setChartAxisBounds(bounds)
  }, [])

  // Check if we can use cached buffer (same time range)
  const canUseCachedBuffer = useCallback(() => {
    if (!cachedTraceBuffer || !cachedTraceTimeRange) return false
    const currentBegin = timeRange.from.toISOString()
    const currentEnd = timeRange.to.toISOString()
    return cachedTraceTimeRange.begin === currentBegin && cachedTraceTimeRange.end === currentEnd
  }, [cachedTraceBuffer, cachedTraceTimeRange, timeRange])

  // Open cached buffer in Perfetto (without regenerating)
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

  // Download cached buffer as file
  const downloadCachedBuffer = useCallback(() => {
    if (!processId || !cachedTraceBuffer) return

    const blob = new Blob([cachedTraceBuffer], { type: 'application/octet-stream' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `trace-${processId}.pb`
    document.body.appendChild(a)
    a.click()
    document.body.removeChild(a)
    URL.revokeObjectURL(url)
    setTraceError(null)
  }, [processId, cachedTraceBuffer])

  const handleOpenInPerfetto = async () => {
    if (!processId) return

    // If we have a cached buffer for the same time range, use it
    if (canUseCachedBuffer()) {
      await openCachedInPerfetto()
      return
    }

    setIsGenerating(true)
    setTraceMode('perfetto')
    setProgress(null)
    setTraceError(null)
    // Clear old cache when generating new trace
    setCachedTraceBuffer(null)
    setCachedTraceTimeRange(null)

    const currentTimeRange = {
      begin: timeRange.from.toISOString(),
      end: timeRange.to.toISOString(),
    }

    const request: GenerateTraceRequest = {
      include_async_spans: true,
      include_thread_spans: true,
      time_range: currentTimeRange,
    }

    try {
      // Generate trace and get buffer
      const buffer = await generateTrace(processId, request, (update) => {
        setProgress(update)
      }, { returnBuffer: true })

      if (!buffer) {
        throw new Error('No trace data received')
      }

      // Cache the buffer for potential retry/download
      setCachedTraceBuffer(buffer)
      setCachedTraceTimeRange(currentTimeRange)

      // Open in Perfetto
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
  }

  const handleDownloadTrace = async () => {
    if (!processId) return

    setIsGenerating(true)
    setTraceMode('download')
    setProgress(null)
    setTraceError(null)

    const request: GenerateTraceRequest = {
      include_async_spans: true,
      include_thread_spans: true,
      time_range: {
        begin: timeRange.from.toISOString(),
        end: timeRange.to.toISOString(),
      },
    }

    try {
      await generateTrace(processId, request, (update) => {
        setProgress(update)
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : 'Unknown error occurred'
      setTraceError(message)
    } finally {
      setIsGenerating(false)
      setTraceMode(null)
      setProgress(null)
    }
  }

  const currentValues = useMemo(
    () => ({
      process_id: processId || '',
      measure_name: selectedMeasure || '',
      bin_interval: binInterval,
    }),
    [processId, selectedMeasure, binInterval]
  )

  const sqlPanel =
    processId && selectedMeasure ? (
      <QueryEditor
        defaultSql={DEFAULT_SQL}
        variables={VARIABLES}
        currentValues={currentValues}
        timeRangeLabel={timeRange.label}
        onRun={handleRunQuery}
        onReset={handleResetQuery}
        isLoading={dataMutation.isPending}
        error={queryError}
        docLink={{
          url: 'https://madesroches.github.io/micromegas/docs/query-guide/schema-reference/#measures',
          label: 'measures schema reference',
        }}
      />
    ) : undefined

  if (!processId) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-app-panel border border-theme-border rounded-lg">
            <AlertCircle className="w-10 h-10 text-accent-error mb-3" />
            <p className="text-theme-text-secondary">No process ID provided</p>
            <AppLink href="/processes" className="text-accent-link hover:underline mt-2">
              Back to Processes
            </AppLink>
          </div>
        </div>
      </PageLayout>
    )
  }

  const noMeasuresAvailable = discoveryDone && measures.length === 0
  const noDataInRange = hasLoaded && chartData.length === 0 && selectedMeasure

  return (
    <PageLayout onRefresh={handleRefresh} rightPanel={sqlPanel}>
      <div className="p-6 flex flex-col">
        {/* Page Header */}
        <div className="mb-5">
          <h1 className="text-2xl font-semibold text-theme-text-primary">Performance Analysis</h1>
          <div className="text-sm text-theme-text-muted font-mono mt-1">
            <CopyableProcessId processId={processId} className="text-sm" />
          </div>
        </div>

        {/* Controls */}
        <div className="flex gap-3 mb-4 items-center flex-wrap">
          <select
            value={selectedMeasure || ''}
            onChange={(e) => updateMeasure(e.target.value)}
            disabled={noMeasuresAvailable || (discoveryMutation.isPending && measures.length === 0)}
            className="min-w-[250px] px-3 py-2 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {measures.length > 0 ? (
              measures.map((m) => (
                <option key={m.name} value={m.name}>
                  {m.name} ({m.unit})
                </option>
              ))
            ) : noMeasuresAvailable ? (
              <option value="">No measures available</option>
            ) : (
              <option value="">Loading measures...</option>
            )}
          </select>

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
            disabled={isGenerating}
            loading={isGenerating}
            loadingLabel={traceMode === 'perfetto' ? 'Opening...' : 'Downloading...'}
            className="ml-auto"
          />

          <span className="text-xs text-theme-text-muted">
            {traceEventCountLoading
              ? 'Loading...'
              : traceEventCount != null
                ? `${traceEventCount.toLocaleString()} thread events`
                : ''}
          </span>
        </div>

        {/* Query Error Banner */}
        {queryError && (
          <ErrorBanner
            title="Query execution failed"
            message={queryError}
            onDismiss={() => setQueryError(null)}
            onRetry={handleRefresh}
          />
        )}

        {/* Trace Error Banner */}
        {traceError && (
          <div className="bg-error-subtle border border-error-border rounded-lg p-4 mb-4">
            <div className="flex items-start gap-3">
              <AlertCircle className="w-5 h-5 text-accent-error flex-shrink-0 mt-0.5" />
              <div className="flex-1 min-w-0">
                <h3 className="text-sm font-medium text-accent-error">
                  {cachedTraceBuffer ? 'Could not open in Perfetto' : 'Trace generation failed'}
                </h3>
                <p className="text-sm text-theme-text-secondary mt-1">{traceError}</p>
                <div className="flex gap-2 mt-3">
                  <button
                    onClick={() => setTraceError(null)}
                    className="px-3 py-1.5 text-sm bg-app-panel border border-theme-border rounded-md text-theme-text-primary hover:bg-app-bg transition-colors"
                  >
                    Dismiss
                  </button>
                  <button
                    onClick={handleOpenInPerfetto}
                    className="px-3 py-1.5 text-sm bg-accent-link text-white rounded-md hover:bg-accent-link/90 transition-colors"
                  >
                    Try Again
                  </button>
                  {cachedTraceBuffer && (
                    <button
                      onClick={downloadCachedBuffer}
                      className="px-3 py-1.5 text-sm bg-app-panel border border-theme-border rounded-md text-theme-text-primary hover:bg-app-bg transition-colors flex items-center gap-1.5"
                    >
                      <Download className="w-4 h-4" />
                      Download Instead
                    </button>
                  )}
                </div>
              </div>
            </div>
          </div>
        )}

        {/* Progress */}
        {isGenerating && (
          <div className="bg-app-panel border border-theme-border rounded-lg p-4 mb-4">
            <div className="flex items-center gap-4">
              <div className="w-5 h-5 border-2 border-theme-border border-t-accent-link rounded-full animate-spin" />
              <span className="text-sm font-medium text-theme-text-primary">
                {traceMode === 'perfetto' ? 'Opening in Perfetto...' : 'Downloading Trace...'}
              </span>
            </div>
            {progress && (
              <p className="text-xs text-theme-text-secondary mt-2">
                {progress.message}
              </p>
            )}
          </div>
        )}

        {/* Chart Area */}
        <div className="h-[350px] mb-4">
          {selectedMeasure && chartData.length > 0 ? (
            <TimeSeriesChart
              data={chartData}
              title={selectedMeasure}
              unit={selectedMeasureInfo?.unit || ''}
              onTimeRangeSelect={handleTimeRangeSelect}
              onWidthChange={handleChartWidthChange}
              onAxisBoundsChange={handleAxisBoundsChange}
            />
          ) : discoveryMutation.isPending ? (
            <div className="h-full flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Discovering measures...</span>
              </div>
            </div>
          ) : noMeasuresAvailable ? (
            <div className="h-full flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
              <div className="flex flex-col items-center text-center px-6">
                <Clock className="w-16 h-16 text-theme-text-muted opacity-50 mb-4" />
                <div className="text-base font-medium text-theme-text-secondary mb-2">
                  No measures for the selected time range
                </div>
                <div className="text-sm text-theme-text-muted max-w-xs">
                  Try expanding the time range to find metrics data.
                </div>
              </div>
            </div>
          ) : dataMutation.isPending && !hasLoaded ? (
            <div className="h-full flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading data...</span>
              </div>
            </div>
          ) : noDataInRange ? (
            <div className="h-full flex flex-col bg-app-panel border border-theme-border rounded-lg">
              <div className="flex justify-between items-center px-4 py-3 border-b border-theme-border">
                <div className="text-base font-medium text-theme-text-primary">
                  {selectedMeasure}{' '}
                  <span className="text-theme-text-muted font-normal">
                    ({selectedMeasureInfo?.unit || ''})
                  </span>
                </div>
              </div>
              <div className="flex-1 flex items-center justify-center">
                <div className="flex flex-col items-center text-center px-6">
                  <Clock className="w-16 h-16 text-theme-text-muted opacity-50 mb-4" />
                  <div className="text-base font-medium text-theme-text-secondary mb-2">
                    No data in time range
                  </div>
                  <div className="text-sm text-theme-text-muted max-w-xs">
                    No measurements found for the selected time range. Try expanding the time range
                    or selecting a different measure.
                  </div>
                </div>
              </div>
            </div>
          ) : null}
        </div>

        {/* Thread Coverage Timeline */}
        {chartTimeRange && threadCoverage.length > 0 && (
          <ThreadCoverageTimeline
            threads={threadCoverage}
            timeRange={chartTimeRange}
            axisBounds={chartAxisBounds}
            onTimeRangeSelect={handleTimeRangeSelect}
          />
        )}

        {/* Hint */}
        {chartData.length > 0 && (
          <div className="text-xs text-theme-text-muted text-center mt-2">
            Drag on the chart or thread coverage to zoom into a time range
          </div>
        )}
      </div>
    </PageLayout>
  )
}

export default function PerformanceAnalysisPage() {
  return (
    <AuthGuard>
      <Suspense
        fallback={
          <PageLayout>
            <div className="p-6">
              <div className="flex items-center justify-center h-64">
                <div className="animate-spin rounded-full h-8 w-8 border-2 border-accent-link border-t-transparent" />
              </div>
            </div>
          </PageLayout>
        }
      >
        <PerformanceAnalysisContent />
      </Suspense>
    </AuthGuard>
  )
}
