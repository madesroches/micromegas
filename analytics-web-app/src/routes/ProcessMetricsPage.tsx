import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useSearchParams, useNavigate, useLocation } from 'react-router-dom'
import { AppLink } from '@/components/AppLink'
import { AlertCircle, Clock } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { MetricsChart } from '@/components/MetricsChart'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useTimeRange } from '@/hooks/useTimeRange'

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

// Convert Arrow timestamp to milliseconds
function arrowTimestampToMs(value: unknown): number {
  if (!value) return 0
  if (value instanceof Date) return value.getTime()
  // Arrow timestamps can be numbers (ms) or BigInt (ns/us)
  if (typeof value === 'number') return value
  if (typeof value === 'bigint') {
    // Assume microseconds, convert to milliseconds
    return Number(value / 1000n)
  }
  // Try parsing as string
  const date = new Date(String(value))
  return isNaN(date.getTime()) ? 0 : date.getTime()
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

function ProcessMetricsContent() {
  const [searchParams] = useSearchParams()
  const navigate = useNavigate()
  const location = useLocation()
  const pathname = location.pathname
  const processId = searchParams.get('process_id')
  const measureParam = searchParams.get('measure')
  const propertiesParam = searchParams.get('properties')
  const { parsed: timeRange, apiTimeRange, setTimeRange } = useTimeRange()

  // Parse selected properties from URL
  const selectedProperties = useMemo(() => {
    if (!propertiesParam) return []
    return propertiesParam.split(',').filter(Boolean)
  }, [propertiesParam])

  const [measures, setMeasures] = useState<Measure[]>([])
  const [selectedMeasure, setSelectedMeasure] = useState<string | null>(measureParam)
  const [chartData, setChartData] = useState<{ time: number; value: number }[]>([])
  const [_processExe, setProcessExe] = useState<string | null>(null)
  const [hasLoaded, setHasLoaded] = useState(false)
  const [discoveryDone, setDiscoveryDone] = useState(false)
  const [chartWidth, setChartWidth] = useState<number>(800)
  const [currentSql, setCurrentSql] = useState<string>(DEFAULT_SQL)

  const discoveryQuery = useStreamQuery()
  const dataQuery = useStreamQuery()
  const processQuery = useStreamQuery()
  const queryError = dataQuery.error?.message ?? discoveryQuery.error?.message ?? null

  const binInterval = useMemo(() => {
    const fromDate = new Date(apiTimeRange.begin)
    const toDate = new Date(apiTimeRange.end)
    const timeSpanMs = toDate.getTime() - fromDate.getTime()
    return calculateBinInterval(timeSpanMs, chartWidth)
  }, [apiTimeRange, chartWidth])

  const selectedMeasureInfo = useMemo(() => {
    return measures.find((m) => m.name === selectedMeasure)
  }, [measures, selectedMeasure])

  // Extract measures from discovery query
  useEffect(() => {
    if (discoveryQuery.isComplete) {
      if (!discoveryQuery.error) {
        const table = discoveryQuery.getTable()
        if (table) {
          const measureList: Measure[] = []
          for (let i = 0; i < table.numRows; i++) {
            const row = table.get(i)
            if (row) {
              measureList.push({
                name: String(row.name ?? ''),
                target: String(row.target ?? ''),
                unit: String(row.unit ?? ''),
              })
            }
          }
          setMeasures(measureList)
          if (measureList.length > 0 && !selectedMeasure) {
            setSelectedMeasure(measureList[0].name)
          }
        }
      }
      setDiscoveryDone(true)
    }
  }, [discoveryQuery.isComplete, discoveryQuery.error, selectedMeasure])

  // Extract chart data from data query
  useEffect(() => {
    if (dataQuery.isComplete && !dataQuery.error) {
      const table = dataQuery.getTable()
      if (table) {
        const points: { time: number; value: number }[] = []
        for (let i = 0; i < table.numRows; i++) {
          const row = table.get(i)
          if (row) {
            points.push({
              time: arrowTimestampToMs(row.time),
              value: Number(row.value),
            })
          }
        }
        setChartData(points)
        setHasLoaded(true)
      }
    }
  }, [dataQuery.isComplete, dataQuery.error])

  // Extract process exe from process query
  useEffect(() => {
    if (processQuery.isComplete && !processQuery.error) {
      const table = processQuery.getTable()
      if (table && table.numRows > 0) {
        const row = table.get(0)
        if (row) {
          setProcessExe(String(row.exe ?? ''))
        }
      }
    }
  }, [processQuery.isComplete, processQuery.error])

  const discoveryExecuteRef = useRef(discoveryQuery.execute)
  discoveryExecuteRef.current = discoveryQuery.execute
  const dataExecuteRef = useRef(dataQuery.execute)
  dataExecuteRef.current = dataQuery.execute
  const processExecuteRef = useRef(processQuery.execute)
  processExecuteRef.current = processQuery.execute

  const currentSqlRef = useRef(currentSql)
  currentSqlRef.current = currentSql

  const loadDiscovery = useCallback(() => {
    if (!processId) return
    discoveryExecuteRef.current({
      sql: DISCOVERY_SQL,
      params: { process_id: processId },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    })
  }, [processId, apiTimeRange])

  const loadData = useCallback(
    (sql: string) => {
      if (!processId || !selectedMeasure) return
      setCurrentSql(sql)
      dataExecuteRef.current({
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

  const updateMeasure = useCallback(
    (measure: string) => {
      setSelectedMeasure(measure)
      const params = new URLSearchParams(searchParams.toString())
      params.set('measure', measure)
      navigate(`${pathname}?${params.toString()}`)
    },
    [searchParams, navigate, pathname]
  )

  const handleAddProperty = useCallback(
    (key: string) => {
      const newProperties = [...selectedProperties, key]
      const params = new URLSearchParams(searchParams.toString())
      params.set('properties', newProperties.join(','))
      navigate(`${pathname}?${params.toString()}`)
    },
    [selectedProperties, searchParams, navigate, pathname]
  )

  const handleRemoveProperty = useCallback(
    (key: string) => {
      const newProperties = selectedProperties.filter((k) => k !== key)
      const params = new URLSearchParams(searchParams.toString())
      if (newProperties.length > 0) {
        params.set('properties', newProperties.join(','))
      } else {
        params.delete('properties')
      }
      navigate(`${pathname}?${params.toString()}`)
    },
    [selectedProperties, searchParams, navigate, pathname]
  )

  const hasLoadedProcessRef = useRef(false)
  useEffect(() => {
    if (processId && !hasLoadedProcessRef.current) {
      hasLoadedProcessRef.current = true
      processExecuteRef.current({
        sql: PROCESS_SQL,
        params: { process_id: processId },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    }
  }, [processId, apiTimeRange])

  const hasLoadedDiscoveryRef = useRef(false)
  useEffect(() => {
    if (processId && !hasLoadedDiscoveryRef.current) {
      hasLoadedDiscoveryRef.current = true
      loadDiscovery()
    }
  }, [processId, loadDiscovery])

  const hasInitialLoadRef = useRef(false)
  useEffect(() => {
    if (discoveryDone && selectedMeasure && processId) {
      // Use DEFAULT_SQL only on initial load, preserve custom SQL for measure changes
      const isInitialLoad = !hasInitialLoadRef.current
      hasInitialLoadRef.current = true
      loadData(isInitialLoad ? DEFAULT_SQL : currentSqlRef.current)
    }
  }, [discoveryDone, selectedMeasure, processId, loadData])

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
    }
  }, [apiTimeRange.begin, apiTimeRange.end, hasLoaded, loadDiscovery])

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
  }, [loadDiscovery])

  const handleTimeRangeSelect = useCallback(
    (from: Date, to: Date) => {
      setTimeRange(from.toISOString(), to.toISOString())
    },
    [setTimeRange]
  )

  const handleChartWidthChange = useCallback((width: number) => {
    setChartWidth(width)
  }, [])

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
        isLoading={dataQuery.isStreaming}
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
      <div className="p-6 flex flex-col h-full">
        <div className="mb-5">
          <h1 className="text-2xl font-semibold text-theme-text-primary">Process Metrics</h1>
          <div className="text-sm text-theme-text-muted font-mono mt-1">
            <CopyableProcessId processId={processId} className="text-sm" />
          </div>
        </div>

        <div className="flex gap-3 mb-4">
          <select
            value={selectedMeasure || ''}
            onChange={(e) => updateMeasure(e.target.value)}
            disabled={noMeasuresAvailable || (discoveryQuery.isStreaming && measures.length === 0)}
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

          <span className="ml-auto text-xs text-theme-text-muted self-center">
            {dataQuery.isStreaming
              ? 'Loading...'
              : noMeasuresAvailable
                ? ''
                : `${chartData.length} data points`}
          </span>
        </div>

        {queryError && (
          <ErrorBanner
            title="Query execution failed"
            message={queryError}
            onRetry={(dataQuery.error?.retryable || discoveryQuery.error?.retryable) ? handleRefresh : undefined}
          />
        )}

        <div className="flex-1 min-h-[400px]">
          {selectedMeasure && chartData.length > 0 ? (
            <MetricsChart
              data={chartData}
              title={selectedMeasure}
              unit={selectedMeasureInfo?.unit || ''}
              processId={processId}
              measureName={selectedMeasure}
              apiTimeRange={apiTimeRange}
              binInterval={binInterval}
              selectedProperties={selectedProperties}
              onAddProperty={handleAddProperty}
              onRemoveProperty={handleRemoveProperty}
              onTimeRangeSelect={handleTimeRangeSelect}
              onWidthChange={handleChartWidthChange}
            />
          ) : discoveryQuery.isStreaming ? (
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
          ) : dataQuery.isStreaming && !hasLoaded ? (
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

        {chartData.length > 0 && (
          <div className="text-xs text-theme-text-muted text-center mt-2">
            Drag on the chart or property timeline to zoom into a time range
          </div>
        )}
      </div>
    </PageLayout>
  )
}

export default function ProcessMetricsPage() {
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
        <ProcessMetricsContent />
      </Suspense>
    </AuthGuard>
  )
}
