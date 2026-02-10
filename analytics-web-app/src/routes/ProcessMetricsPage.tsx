import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { AppLink } from '@/components/AppLink'
import { AlertCircle, Clock } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { MEASURES_SCHEMA_URL } from '@/components/DocumentationLink'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { ParseErrorWarning } from '@/components/ParseErrorWarning'
import { MetricsChart } from '@/components/MetricsChart'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useMetricsData } from '@/hooks/useMetricsData'
import { useScreenConfig } from '@/hooks/useScreenConfig'
import { usePageTitle } from '@/hooks/usePageTitle'
import { parseTimeRange, getTimeRangeForApi } from '@/lib/time-range'
import { timestampToMs } from '@/lib/arrow-utils'
import { extractPropertiesFromRows, createPropertyTimelineGetter, ExtractedPropertyData } from '@/lib/property-utils'
import { useDefaultDataSource } from '@/hooks/useDefaultDataSource'
import type { ProcessMetricsConfig } from '@/lib/screen-config'

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

const VARIABLES = [
  { name: 'process_id', description: 'Current process ID' },
  { name: 'measure_name', description: 'Selected measure name' },
  { name: 'bin_interval', description: 'Time bucket size for downsampling' },
]

// Default config for ProcessMetricsPage
const DEFAULT_CONFIG: ProcessMetricsConfig = {
  processId: '',
  timeRangeFrom: 'now-1h',
  timeRangeTo: 'now',
  selectedMeasure: undefined,
  selectedProperties: [],
}

// URL builder for ProcessMetricsPage - builds query string from config
const buildUrl = (cfg: ProcessMetricsConfig): string => {
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

function ProcessMetricsContent() {
  usePageTitle('Process Metrics')

  const { name: defaultDataSource, error: dataSourceError } = useDefaultDataSource()

  // Use the new config-driven pattern
  const { config, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl)
  const processId = config.processId
  const selectedProperties = useMemo(() => config.selectedProperties ?? [], [config.selectedProperties])

  // Local state for UI
  const [measures, setMeasures] = useState<Measure[]>([])
  const [selectedMeasure, setSelectedMeasure] = useState<string | null>(config.selectedMeasure ?? null)
  const [discoveryDone, setDiscoveryDone] = useState(false)
  const [chartWidth, setChartWidth] = useState<number>(800)
  const [isCustomQuery, setIsCustomQuery] = useState(false)
  const [customChartData, setCustomChartData] = useState<{ time: number; value: number }[]>([])
  const [customPropertyData, setCustomPropertyData] = useState<ExtractedPropertyData>({ availableKeys: [], rawData: new Map(), errors: [] })

  // Query hooks for discovery and custom queries
  const discoveryQuery = useStreamQuery()
  const customQuery = useStreamQuery()

  // Compute API time range from config
  const apiTimeRange = useMemo(() => {
    try {
      return getTimeRangeForApi(config.timeRangeFrom ?? 'now-1h', config.timeRangeTo ?? 'now')
    } catch {
      return getTimeRangeForApi('now-1h', 'now')
    }
  }, [config.timeRangeFrom, config.timeRangeTo])

  // Compute display label for time range
  const timeRangeLabel = useMemo(() => {
    try {
      return parseTimeRange(config.timeRangeFrom ?? 'now-1h', config.timeRangeTo ?? 'now').label
    } catch {
      return 'Last 1 hour'
    }
  }, [config.timeRangeFrom, config.timeRangeTo])

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
    dataSource: defaultDataSource,
  })

  // Use unified data or custom query data
  const chartData = isCustomQuery ? customChartData : metricsData.chartData
  const isLoading = isCustomQuery ? customQuery.isStreaming : metricsData.isLoading
  const hasLoaded = isCustomQuery ? customQuery.isComplete : metricsData.isComplete
  const queryError = customQuery.error?.message ?? discoveryQuery.error?.message ?? metricsData.error
  // Show loading when discovery is done, measure selected, but data hasn't loaded yet
  const showDataLoading = isLoading || (discoveryDone && selectedMeasure && !hasLoaded && chartData.length === 0)

  // Compute time range in milliseconds for property timeline
  const timeRangeMs = useMemo(() => ({
    begin: new Date(apiTimeRange.begin).getTime(),
    end: new Date(apiTimeRange.end).getTime(),
  }), [apiTimeRange.begin, apiTimeRange.end])

  // Use custom or unified property data based on query mode
  const availablePropertyKeys = isCustomQuery ? customPropertyData.availableKeys : metricsData.availablePropertyKeys
  const getPropertyTimeline = useMemo(
    () => isCustomQuery
      ? createPropertyTimelineGetter(customPropertyData.rawData, timeRangeMs)
      : metricsData.getPropertyTimeline,
    [isCustomQuery, customPropertyData.rawData, timeRangeMs, metricsData.getPropertyTimeline]
  )
  const propertyParseErrors = isCustomQuery ? customPropertyData.errors : metricsData.propertyParseErrors

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
          // Auto-select first measure if none specified
          if (measureList.length > 0 && !selectedMeasure) {
            const autoMeasure = measureList[0].name
            setSelectedMeasure(autoMeasure)
            // Update config to keep URL in sync (replace to avoid history entry)
            updateConfig({ selectedMeasure: autoMeasure }, { replace: true })
          }
        }
      }
      setDiscoveryDone(true)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- Only react to completion/error, not the full hook object
  }, [discoveryQuery.isComplete, discoveryQuery.error, selectedMeasure, updateConfig])

  // Extract custom query data (chart + properties if present)
  useEffect(() => {
    if (customQuery.isComplete && !customQuery.error) {
      const table = customQuery.getTable()
      if (table) {
        const points: { time: number; value: number }[] = []
        const propsRows: { time: number; properties: string | null }[] = []
        const hasPropertiesColumn = table.schema.fields.some(f => f.name === 'properties')

        for (let i = 0; i < table.numRows; i++) {
          const row = table.get(i)
          if (row) {
            const time = timestampToMs(row.time)
            points.push({ time, value: Number(row.value) })
            if (hasPropertiesColumn) {
              propsRows.push({ time, properties: row.properties != null ? String(row.properties) : null })
            }
          }
        }

        setCustomChartData(points)
        setCustomPropertyData(
          hasPropertiesColumn
            ? extractPropertiesFromRows(propsRows)
            : { availableKeys: [], rawData: new Map(), errors: [] }
        )
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- Only react to completion/error, not the full hook object
  }, [customQuery.isComplete, customQuery.error])

  const discoveryExecuteRef = useRef(discoveryQuery.execute)
  discoveryExecuteRef.current = discoveryQuery.execute

  const loadDiscovery = useCallback(() => {
    if (!processId) return
    discoveryExecuteRef.current({
      sql: DISCOVERY_SQL,
      params: { process_id: processId },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
      dataSource: defaultDataSource,
    })
  }, [processId, apiTimeRange, defaultDataSource])

  // Update measure in config with replace (editing, not navigational)
  const updateMeasure = useCallback(
    (measure: string) => {
      setSelectedMeasure(measure)
      setIsCustomQuery(false)
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

  const hasLoadedDiscoveryRef = useRef(false)
  useEffect(() => {
    if (processId && defaultDataSource && !hasLoadedDiscoveryRef.current) {
      hasLoadedDiscoveryRef.current = true
      loadDiscovery()
    }
  }, [processId, defaultDataSource, loadDiscovery])

  // Trigger unified query when discovery is done and measure is selected
  const metricsDataExecuteRef = useRef(metricsData.execute)
  metricsDataExecuteRef.current = metricsData.execute

  useEffect(() => {
    if (discoveryDone && selectedMeasure && processId && !isCustomQuery) {
      metricsDataExecuteRef.current()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- Use primitive deps to avoid object comparison issues
  }, [discoveryDone, selectedMeasure, processId, isCustomQuery, binInterval, apiTimeRange.begin, apiTimeRange.end])

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

  // Time range changes create history entries (navigational)
  const handleTimeRangeChange = useCallback(
    (from: string, to: string) => {
      updateConfig({ timeRangeFrom: from, timeRangeTo: to })
    },
    [updateConfig]
  )

  const handleRunQuery = useCallback(
    (sql: string) => {
      setIsCustomQuery(true)
      customQuery.execute({
        sql,
        params: {
          process_id: processId || '',
          measure_name: selectedMeasure || '',
          bin_interval: binInterval,
        },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
        dataSource: defaultDataSource,
      })
    },
    [processId, selectedMeasure, binInterval, apiTimeRange, defaultDataSource, customQuery]
  )

  const handleResetQuery = useCallback(() => {
    setIsCustomQuery(false)
  }, [])

  const handleRefresh = useCallback(() => {
    hasLoadedDiscoveryRef.current = false
    loadDiscovery()
  }, [loadDiscovery])

  const handleTimeRangeSelect = useCallback(
    (from: Date, to: Date) => {
      updateConfig({ timeRangeFrom: from.toISOString(), timeRangeTo: to.toISOString() })
    },
    [updateConfig]
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
        timeRangeLabel={timeRangeLabel}
        onRun={handleRunQuery}
        onReset={handleResetQuery}
        isLoading={isLoading}
        error={queryError}
        docLink={{
          url: MEASURES_SCHEMA_URL,
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
            {isLoading
              ? 'Loading...'
              : noMeasuresAvailable
                ? ''
                : `${chartData.length} data points`}
          </span>
        </div>

        {dataSourceError && (
          <ErrorBanner
            title="Data source error"
            message={dataSourceError}
          />
        )}

        {queryError && (
          <ErrorBanner
            title="Query execution failed"
            message={queryError}
            onRetry={(customQuery.error?.retryable || discoveryQuery.error?.retryable) ? handleRefresh : undefined}
          />
        )}

        <ParseErrorWarning errors={propertyParseErrors} />

        <div className="flex-1 min-h-[400px]">
          {selectedMeasure && chartData.length > 0 ? (
            <MetricsChart
              data={chartData}
              title={selectedMeasure}
              unit={selectedMeasureInfo?.unit || ''}
              availablePropertyKeys={availablePropertyKeys}
              getPropertyTimeline={getPropertyTimeline}
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
        <ProcessMetricsContent key={processId} />
      </Suspense>
    </AuthGuard>
  )
}
