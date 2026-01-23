import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { AppLink } from '@/components/AppLink'
import { SplitButton } from '@/components/ui/SplitButton'
import { AlertCircle, Clock, Download, ExternalLink } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { ChartAxisBounds } from '@/components/XYChart'
import { MetricsChart, ScaleMode } from '@/components/MetricsChart'
import { ThreadCoverageTimeline } from '@/components/ThreadCoverageTimeline'
import { generateTrace } from '@/lib/api'
import { executeStreamQuery } from '@/lib/arrow-stream'
import { timestampToMs } from '@/lib/arrow-utils'
import { openInPerfetto, PerfettoError } from '@/lib/perfetto'
import { parseTimeRange, getTimeRangeForApi } from '@/lib/time-range'
import { useScreenConfig } from '@/hooks/useScreenConfig'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useMetricsData } from '@/hooks/useMetricsData'
import { GenerateTraceRequest, ProgressUpdate, ThreadCoverage } from '@/types'
import type { PerformanceAnalysisConfig } from '@/lib/screen-config'

const DISCOVERY_SQL = `SELECT DISTINCT name, target, unit
FROM view_instance('measures', '$process_id')
ORDER BY name`

const DEFAULT_SQL = `SELECT
  date_bin(INTERVAL '$bin_interval', time) as time,
  max(value) as value,
  jsonb_format_json(first_value(properties) FILTER (WHERE properties IS NOT NULL)) as properties
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

// Default config for PerformanceAnalysisPage
const DEFAULT_CONFIG: PerformanceAnalysisConfig = {
  processId: '',
  timeRangeFrom: 'now-1h',
  timeRangeTo: 'now',
  selectedMeasure: undefined,
  selectedProperties: [],
  scaleMode: 'p99',
}

// URL builder for PerformanceAnalysisPage - builds query string from config
const buildUrl = (cfg: PerformanceAnalysisConfig): string => {
  const params = new URLSearchParams()
  if (cfg.processId) params.set('process_id', cfg.processId)
  if (cfg.timeRangeFrom && cfg.timeRangeFrom !== DEFAULT_CONFIG.timeRangeFrom) {
    params.set('from', cfg.timeRangeFrom)
  }
  if (cfg.timeRangeTo && cfg.timeRangeTo !== DEFAULT_CONFIG.timeRangeTo) {
    params.set('to', cfg.timeRangeTo)
  }
  if (cfg.selectedMeasure) params.set('measure', cfg.selectedMeasure)
  if (cfg.selectedProperties && cfg.selectedProperties.length > 0) {
    params.set('properties', cfg.selectedProperties.join(','))
  }
  if (cfg.scaleMode && cfg.scaleMode !== 'p99') params.set('scale', cfg.scaleMode)
  const qs = params.toString()
  return qs ? `?${qs}` : ''
}

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
  usePageTitle('Performance Analysis')

  // Use the new config-driven pattern
  const { config, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl)
  const processId = config.processId
  const selectedProperties = useMemo(() => config.selectedProperties ?? [], [config.selectedProperties])
  const scaleMode: ScaleMode = (config.scaleMode ?? 'p99') as ScaleMode

  // Compute API time range from config
  const apiTimeRange = useMemo(() => {
    try {
      return getTimeRangeForApi(config.timeRangeFrom ?? 'now-1h', config.timeRangeTo ?? 'now')
    } catch {
      return getTimeRangeForApi('now-1h', 'now')
    }
  }, [config.timeRangeFrom, config.timeRangeTo])

  // Compute display label and dates for time range
  const timeRangeParsed = useMemo(() => {
    try {
      return parseTimeRange(config.timeRangeFrom ?? 'now-1h', config.timeRangeTo ?? 'now')
    } catch {
      return { label: 'Last 1 hour', from: new Date(Date.now() - 3600000), to: new Date() }
    }
  }, [config.timeRangeFrom, config.timeRangeTo])

  const [measures, setMeasures] = useState<Measure[]>([])
  const [selectedMeasure, setSelectedMeasure] = useState<string | null>(config.selectedMeasure ?? null)
  const [queryError, setQueryError] = useState<string | null>(null)
  const [_processExe, setProcessExe] = useState<string | null>(null)
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
  const [cachedTraceBuffer, setCachedTraceBuffer] = useState<ArrayBuffer | null>(null)
  const [cachedTraceTimeRange, setCachedTraceTimeRange] = useState<{ begin: string; end: string } | null>(null)
  const [_currentSql, setCurrentSql] = useState<string>(DEFAULT_SQL)
  const [isCustomQuery, setIsCustomQuery] = useState(false)
  const [customChartData, setCustomChartData] = useState<{ time: number; value: number }[]>([])

  const binInterval = useMemo(() => {
    const fromDate = new Date(apiTimeRange.begin)
    const toDate = new Date(apiTimeRange.end)
    const timeSpanMs = toDate.getTime() - fromDate.getTime()
    return calculateBinInterval(timeSpanMs, chartWidth)
  }, [apiTimeRange, chartWidth])

  // Unified metrics data hook (Model layer)
  const metricsData = useMetricsData({
    processId,
    measureName: selectedMeasure,
    binInterval,
    apiTimeRange,
    enabled: !!processId && !!selectedMeasure,
  })

  // Use unified data or custom query data
  const chartData = isCustomQuery ? customChartData : metricsData.chartData
  const dataLoading = isCustomQuery ? false : metricsData.isLoading
  const hasLoaded = isCustomQuery ? customChartData.length > 0 || queryError !== null : metricsData.isComplete

  const selectedMeasureInfo = useMemo(() => {
    return measures.find((m) => m.name === selectedMeasure)
  }, [measures, selectedMeasure])

  const chartTimeRange = useMemo(() => {
    if (chartData.length === 0) return null
    return {
      from: Math.min(...chartData.map((d) => d.time)),
      to: Math.max(...chartData.map((d) => d.time)),
    }
  }, [chartData])

  const [discoveryLoading, setDiscoveryLoading] = useState(false)
  const [customQueryLoading, setCustomQueryLoading] = useState(false)

  const loadDiscovery = useCallback(async () => {
    if (!processId) return
    setDiscoveryLoading(true)

    try {
      const { batches, error } = await executeStreamQuery({
        sql: DISCOVERY_SQL,
        params: { process_id: processId },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })

      if (error) {
        setQueryError(error.message)
        setDiscoveryDone(true)
        setDiscoveryLoading(false)
        return
      }

      const measureList: Measure[] = []
      for (const batch of batches) {
        for (let i = 0; i < batch.numRows; i++) {
          const row = batch.get(i)
          if (row) {
            measureList.push({
              name: String(row.name ?? ''),
              target: String(row.target ?? ''),
              unit: String(row.unit ?? ''),
            })
          }
        }
      }

      setMeasures(measureList)
      setDiscoveryDone(true)

      // Auto-select measure if none specified - use DeltaTime if available, else first
      if (measureList.length > 0 && !selectedMeasure) {
        const deltaTime = measureList.find((m) => m.name === 'DeltaTime')
        const autoMeasure = deltaTime ? deltaTime.name : measureList[0].name
        setSelectedMeasure(autoMeasure)
        // Update config to keep URL in sync (replace to avoid history entry)
        updateConfig({ selectedMeasure: autoMeasure }, { replace: true })
      }
    } catch (err) {
      setQueryError(err instanceof Error ? err.message : 'Unknown error')
      setDiscoveryDone(true)
    } finally {
      setDiscoveryLoading(false)
    }
  }, [processId, apiTimeRange, selectedMeasure, updateConfig])

  const loadCustomQuery = useCallback(
    async (sql: string) => {
      if (!processId || !selectedMeasure) return
      setQueryError(null)
      setCurrentSql(sql)
      setCustomQueryLoading(true)
      setIsCustomQuery(true)

      try {
        const { batches, error } = await executeStreamQuery({
          sql,
          params: {
            process_id: processId,
            measure_name: selectedMeasure,
            bin_interval: binInterval,
          },
          begin: apiTimeRange.begin,
          end: apiTimeRange.end,
        })

        if (error) {
          setQueryError(error.message)
          setCustomQueryLoading(false)
          return
        }

        const points: { time: number; value: number }[] = []
        for (const batch of batches) {
          for (let i = 0; i < batch.numRows; i++) {
            const row = batch.get(i)
            if (row) {
              points.push({
                time: timestampToMs(row.time),
                value: Number(row.value),
              })
            }
          }
        }

        setCustomChartData(points)
      } catch (err) {
        setQueryError(err instanceof Error ? err.message : 'Unknown error')
      } finally {
        setCustomQueryLoading(false)
      }
    },
    [processId, selectedMeasure, binInterval, apiTimeRange]
  )

  // Ref to always call the latest loadDiscovery without causing effect re-runs
  const loadDiscoveryRef = useRef<(() => Promise<void>) | null>(null)
  loadDiscoveryRef.current = loadDiscovery

  const loadThreadCoverage = useCallback(async () => {
    if (!processId) return

    // Load thread coverage
    try {
      const { batches, error } = await executeStreamQuery({
        sql: THREAD_COVERAGE_SQL,
        params: { process_id: processId },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })

      if (error) {
        console.error('Failed to fetch thread coverage:', error.message)
        setThreadCoverage([])
      } else {
        const threadMap = new Map<string, ThreadCoverage>()

        for (const batch of batches) {
          for (let i = 0; i < batch.numRows; i++) {
            const row = batch.get(i)
            if (row) {
              const streamId = String(row.stream_id ?? '')
              const threadName = String(row.thread_name ?? 'unknown')
              const beginTime = timestampToMs(row.begin_time)
              const endTime = timestampToMs(row.end_time)

              if (!threadMap.has(streamId)) {
                threadMap.set(streamId, {
                  streamId,
                  threadName,
                  segments: [],
                })
              }
              threadMap.get(streamId)!.segments.push({ begin: beginTime, end: endTime })
            }
          }
        }

        const threads = Array.from(threadMap.values())
        threads.sort((a, b) => a.threadName.localeCompare(b.threadName))
        for (const thread of threads) {
          thread.segments.sort((a, b) => a.begin - b.begin)
        }

        setThreadCoverage(threads)
      }
    } catch (err) {
      console.error('Failed to fetch thread coverage:', err)
      setThreadCoverage([])
    }

    // Load trace event count
    setTraceEventCountLoading(true)
    try {
      const { batches, error } = await executeStreamQuery({
        sql: TRACE_EVENTS_COUNT_SQL,
        params: { process_id: processId },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })

      if (error) {
        console.error('Failed to fetch trace event count:', error.message)
        setTraceEventCount(0)
      } else {
        let eventCount = 0
        for (const batch of batches) {
          if (batch.numRows > 0) {
            const row = batch.get(0)
            if (row && row.event_count != null) {
              eventCount = Number(row.event_count)
            }
          }
        }
        setTraceEventCount(eventCount)
      }
    } catch (err) {
      console.error('Failed to fetch trace event count:', err)
      setTraceEventCount(0)
    } finally {
      setTraceEventCountLoading(false)
    }
  }, [processId, apiTimeRange])

  // Ref to always call the latest loadThreadCoverage without causing effect re-runs
  const loadThreadCoverageRef = useRef<(() => Promise<void>) | null>(null)
  loadThreadCoverageRef.current = loadThreadCoverage

  // Update measure in config with replace (editing, not navigational)
  const updateMeasure = useCallback(
    (measure: string) => {
      setSelectedMeasure(measure)
      setIsCustomQuery(false)
      setCurrentSql(DEFAULT_SQL)
      updateConfig({ selectedMeasure: measure }, { replace: true })
    },
    [updateConfig]
  )

  const handleAddProperty = useCallback(
    (key: string) => {
      const newProperties = [...selectedProperties, key]
      updateConfig({ selectedProperties: newProperties }, { replace: true })
    },
    [selectedProperties, updateConfig]
  )

  const handleRemoveProperty = useCallback(
    (key: string) => {
      const newProperties = selectedProperties.filter((k) => k !== key)
      updateConfig({ selectedProperties: newProperties.length > 0 ? newProperties : undefined }, { replace: true })
    },
    [selectedProperties, updateConfig]
  )

  const handleScaleModeChange = useCallback(
    (mode: ScaleMode) => {
      updateConfig({ scaleMode: mode === 'p99' ? undefined : mode }, { replace: true })
    },
    [updateConfig]
  )

  const hasLoadedProcessRef = useRef(false)
  useEffect(() => {
    if (processId && !hasLoadedProcessRef.current) {
      hasLoadedProcessRef.current = true
      executeStreamQuery({
        sql: PROCESS_SQL,
        params: { process_id: processId },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      }).then(({ batches }) => {
        for (const batch of batches) {
          if (batch.numRows > 0) {
            const row = batch.get(0)
            if (row) {
              setProcessExe(String(row.exe ?? ''))
            }
          }
        }
      })
    }
  }, [processId, apiTimeRange])

  const hasLoadedDiscoveryRef = useRef(false)
  useEffect(() => {
    if (processId && !hasLoadedDiscoveryRef.current) {
      hasLoadedDiscoveryRef.current = true
      // Use refs to avoid re-running this effect when callback identities change
      loadDiscoveryRef.current?.()
      loadThreadCoverageRef.current?.()
    }
  }, [processId])

  // Trigger unified query when discovery is done and measure is selected
  const metricsDataExecuteRef = useRef(metricsData.execute)
  metricsDataExecuteRef.current = metricsData.execute

  useEffect(() => {
    if (discoveryDone && selectedMeasure && processId && !isCustomQuery) {
      metricsDataExecuteRef.current()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- Use primitive deps to avoid object comparison issues
  }, [discoveryDone, selectedMeasure, processId, isCustomQuery, binInterval, apiTimeRange.begin, apiTimeRange.end])

  // Re-execute queries when time range changes
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
      // Use refs to avoid re-running this effect when callback identities change
      loadDiscoveryRef.current?.()
      loadThreadCoverageRef.current?.()
    }
  }, [apiTimeRange.begin, apiTimeRange.end, hasLoaded])

  const handleRunQuery = useCallback(
    (sql: string) => {
      loadCustomQuery(sql)
    },
    [loadCustomQuery]
  )

  const handleResetQuery = useCallback(() => {
    setCurrentSql(DEFAULT_SQL)
    setIsCustomQuery(false)
  }, [])

  const handleRefresh = useCallback(() => {
    hasLoadedDiscoveryRef.current = false
    loadDiscoveryRef.current?.()
    loadThreadCoverageRef.current?.()
  }, [])

  // Time range changes create history entries (navigational)
  const handleTimeRangeChange = useCallback(
    (from: string, to: string) => {
      updateConfig({ timeRangeFrom: from, timeRangeTo: to })
    },
    [updateConfig]
  )

  const handleTimeRangeSelect = useCallback(
    (from: Date, to: Date) => {
      updateConfig({ timeRangeFrom: from.toISOString(), timeRangeTo: to.toISOString() })
    },
    [updateConfig]
  )

  const handleChartWidthChange = useCallback((width: number) => {
    setChartWidth(width)
  }, [])

  const handleAxisBoundsChange = useCallback((bounds: ChartAxisBounds) => {
    setChartAxisBounds(bounds)
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

    const currentTimeRange = {
      begin: timeRangeParsed.from.toISOString(),
      end: timeRangeParsed.to.toISOString(),
    }

    const request: GenerateTraceRequest = {
      include_async_spans: true,
      include_thread_spans: true,
      time_range: currentTimeRange,
    }

    try {
      const buffer = await generateTrace(processId, request, (update) => {
        setProgress(update)
      }, { returnBuffer: true })

      if (!buffer) {
        throw new Error('No trace data received')
      }

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
        begin: timeRangeParsed.from.toISOString(),
        end: timeRangeParsed.to.toISOString(),
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

  const isLoading = dataLoading || customQueryLoading
  // Show loading when discovery is done, measure selected, but data hasn't loaded yet
  const showDataLoading = isLoading || (discoveryDone && selectedMeasure && !hasLoaded && chartData.length === 0)

  const sqlPanel =
    processId && selectedMeasure ? (
      <QueryEditor
        defaultSql={DEFAULT_SQL}
        variables={VARIABLES}
        currentValues={currentValues}
        timeRangeLabel={timeRangeParsed.label}
        onRun={handleRunQuery}
        onReset={handleResetQuery}
        isLoading={isLoading}
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
    <PageLayout
      onRefresh={handleRefresh}
      rightPanel={sqlPanel}
      timeRangeControl={{
        timeRangeFrom: config.timeRangeFrom ?? 'now-1h',
        timeRangeTo: config.timeRangeTo ?? 'now',
        onTimeRangeChange: handleTimeRangeChange,
      }}
      processId={processId}
    >
      <div className="p-6 flex flex-col">
        <div className="mb-5">
          <h1 className="text-2xl font-semibold text-theme-text-primary">Performance Analysis</h1>
          <div className="text-sm text-theme-text-muted font-mono mt-1">
            <CopyableProcessId processId={processId} className="text-sm" />
          </div>
        </div>

        <div className="flex gap-3 mb-4 items-center flex-wrap">
          <select
            value={selectedMeasure || ''}
            onChange={(e) => updateMeasure(e.target.value)}
            disabled={noMeasuresAvailable || (discoveryLoading && measures.length === 0)}
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

        {queryError && (
          <ErrorBanner
            title="Query execution failed"
            message={queryError}
            onDismiss={() => setQueryError(null)}
            onRetry={handleRefresh}
          />
        )}

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

        <div className="mb-4">
          {selectedMeasure && chartData.length > 0 ? (
            <MetricsChart
              data={chartData}
              title={selectedMeasure}
              unit={selectedMeasureInfo?.unit || ''}
              availablePropertyKeys={metricsData.availablePropertyKeys}
              getPropertyTimeline={metricsData.getPropertyTimeline}
              selectedProperties={selectedProperties}
              onAddProperty={handleAddProperty}
              onRemoveProperty={handleRemoveProperty}
              scaleMode={scaleMode}
              onScaleModeChange={handleScaleModeChange}
              onTimeRangeSelect={handleTimeRangeSelect}
              onWidthChange={handleChartWidthChange}
              onAxisBoundsChange={handleAxisBoundsChange}
            />
          ) : discoveryLoading ? (
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
          ) : showDataLoading ? (
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

        {chartTimeRange && threadCoverage.length > 0 && (
          <ThreadCoverageTimeline
            threads={threadCoverage}
            timeRange={chartTimeRange}
            axisBounds={chartAxisBounds}
            onTimeRangeSelect={handleTimeRangeSelect}
          />
        )}

        {chartData.length > 0 && (
          <div className="text-xs text-theme-text-muted text-center mt-2">
            Drag on the chart, property timeline, or thread coverage to zoom into a time range
          </div>
        )}
      </div>
    </PageLayout>
  )
}

export default function PerformanceAnalysisPage() {
  // Read processId from URL to use as key for remounting content
  const [searchParams] = useSearchParams()
  const processId = searchParams.get('process_id')

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
        {/* Key on processId to force remount when switching processes */}
        <PerformanceAnalysisContent key={processId} />
      </Suspense>
    </AuthGuard>
  )
}
