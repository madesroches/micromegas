import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { AppLink } from '@/components/AppLink'
import { SplitButton } from '@/components/ui/SplitButton'
import { AlertCircle, Download, ExternalLink } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { MEASURES_SCHEMA_URL } from '@/components/DocumentationLink'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { ParseErrorWarning } from '@/components/ParseErrorWarning'
import { ChartAxisBounds } from '@/components/XYChart'
import { ScaleMode } from '@/components/MetricsChart'
import { parseTimeRange, getTimeRangeForApi } from '@/lib/time-range'
import { useScreenConfig } from '@/hooks/useScreenConfig'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useDefaultDataSource } from '@/hooks/useDefaultDataSource'
import {
  DEFAULT_SQL,
  VARIABLES,
  DEFAULT_CONFIG,
  buildUrl,
  calculateBinInterval,
  type Measure,
} from './perf-analysis/queries'
import { MeasureDiscovery } from './perf-analysis/MeasureDiscovery'
import {
  PerformanceMetricsChart,
  type CustomQueryHandle,
  type MetricsViewState,
} from './perf-analysis/PerformanceMetricsChart'
import { ThreadCoveragePanel } from './perf-analysis/ThreadCoveragePanel'
import { usePerfettoTrace } from './perf-analysis/usePerfettoTrace'

const EMPTY_METRICS_VIEW: MetricsViewState = {
  hasLoaded: false,
  isLoading: false,
  chartTimeRange: null,
  chartDataLength: 0,
  propertyParseErrors: [],
}

function PerformanceAnalysisContent() {
  usePageTitle('Performance Analysis')

  const { name: defaultDataSource, error: dataSourceError } = useDefaultDataSource()

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

  // Shared state read across the gate and multiple sections.
  const [measures, setMeasures] = useState<Measure[]>([])
  const [selectedMeasure, setSelectedMeasure] = useState<string | null>(config.selectedMeasure ?? null)
  const [queryError, setQueryError] = useState<string | null>(null)
  const [discoveryDone, setDiscoveryDone] = useState(false)
  const [discoveryLoading, setDiscoveryLoading] = useState(false)
  const [chartWidth, setChartWidth] = useState<number>(800)
  const [chartAxisBounds, setChartAxisBounds] = useState<ChartAxisBounds | null>(null)
  const [traceEventCount, setTraceEventCount] = useState<number | null>(null)
  const [traceEventCountLoading, setTraceEventCountLoading] = useState(false)
  const [metricsView, setMetricsView] = useState<MetricsViewState>(EMPTY_METRICS_VIEW)

  const binInterval = useMemo(() => {
    const fromDate = new Date(apiTimeRange.begin)
    const toDate = new Date(apiTimeRange.end)
    const timeSpanMs = toDate.getTime() - fromDate.getTime()
    return calculateBinInterval(timeSpanMs, chartWidth)
  }, [apiTimeRange, chartWidth])

  // Self-contained Perfetto trace generation (button + banners).
  const trace = usePerfettoTrace({ processId, timeRangeParsed, dataSource: defaultDataSource })

  // Loaders registered by the child sections so the page can gate re-fetches.
  const loadDiscoveryRef = useRef<(() => Promise<void>) | null>(null)
  const loadThreadCoverageRef = useRef<(() => Promise<void>) | null>(null)
  const customQueryRef = useRef<CustomQueryHandle | null>(null)

  // Update measure in config with replace (editing, not navigational)
  const onAutoSelect = useCallback(
    (measure: string) => {
      setSelectedMeasure(measure)
      updateConfig({ selectedMeasure: measure }, { replace: true })
    },
    [updateConfig]
  )

  const onMeasureChange = useCallback(
    (measure: string) => {
      setSelectedMeasure(measure)
      customQueryRef.current?.reset()
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

  const hasLoaded = metricsView.hasLoaded

  const hasLoadedDiscoveryRef = useRef(false)
  useEffect(() => {
    if (processId && defaultDataSource && !hasLoadedDiscoveryRef.current) {
      hasLoadedDiscoveryRef.current = true
      // Use refs to avoid re-running this effect when callback identities change
      loadDiscoveryRef.current?.()
      loadThreadCoverageRef.current?.()
    }
  }, [processId, defaultDataSource])

  // Re-execute queries when time range changes (gated on metrics completion so
  // discovery + thread-coverage re-fetch in lock-step with the metrics chart).
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

  const handleRunQuery = useCallback((sql: string) => {
    customQueryRef.current?.run(sql)
  }, [])

  const handleResetQuery = useCallback(() => {
    customQueryRef.current?.reset()
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

  const onViewStateChange = useCallback((state: MetricsViewState) => {
    setMetricsView(state)
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
        timeRangeLabel={timeRangeParsed.label}
        onRun={handleRunQuery}
        onReset={handleResetQuery}
        isLoading={metricsView.isLoading}
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
          <MeasureDiscovery
            processId={processId}
            timeRange={apiTimeRange}
            dataSource={defaultDataSource}
            selectedMeasure={selectedMeasure}
            measures={measures}
            discoveryLoading={discoveryLoading}
            noMeasuresAvailable={noMeasuresAvailable}
            setMeasures={setMeasures}
            setDiscoveryDone={setDiscoveryDone}
            setDiscoveryLoading={setDiscoveryLoading}
            onError={setQueryError}
            onAutoSelect={onAutoSelect}
            onMeasureChange={onMeasureChange}
            loadRef={loadDiscoveryRef}
          />

          <SplitButton
            primaryLabel="Open in Perfetto"
            primaryIcon={<ExternalLink className="w-4 h-4" />}
            onPrimaryClick={trace.handleOpenInPerfetto}
            secondaryActions={[
              {
                label: 'Download',
                icon: <Download className="w-4 h-4" />,
                onClick: trace.handleDownloadTrace,
              },
            ]}
            disabled={trace.isGenerating}
            loading={trace.isGenerating}
            loadingLabel={trace.traceMode === 'perfetto' ? 'Opening...' : 'Downloading...'}
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
            onDismiss={() => setQueryError(null)}
            onRetry={handleRefresh}
          />
        )}

        <ParseErrorWarning errors={metricsView.propertyParseErrors} />

        {trace.traceError && (
          <div className="bg-error-subtle border border-error-border rounded-lg p-4 mb-4">
            <div className="flex items-start gap-3">
              <AlertCircle className="w-5 h-5 text-accent-error flex-shrink-0 mt-0.5" />
              <div className="flex-1 min-w-0">
                <h3 className="text-sm font-medium text-accent-error">
                  {trace.cachedTraceBuffer ? 'Could not open in Perfetto' : 'Trace generation failed'}
                </h3>
                <p className="text-sm text-theme-text-secondary mt-1">{trace.traceError}</p>
                <div className="flex gap-2 mt-3">
                  <button
                    onClick={trace.dismissTraceError}
                    className="px-3 py-1.5 text-sm bg-app-panel border border-theme-border rounded-md text-theme-text-primary hover:bg-app-bg transition-colors"
                  >
                    Dismiss
                  </button>
                  <button
                    onClick={trace.handleOpenInPerfetto}
                    className="px-3 py-1.5 text-sm bg-accent-link text-white rounded-md hover:bg-accent-link/90 transition-colors"
                  >
                    Try Again
                  </button>
                  {trace.cachedTraceBuffer && (
                    <button
                      onClick={trace.downloadCachedBuffer}
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

        {trace.isGenerating && (
          <div className="bg-app-panel border border-theme-border rounded-lg p-4 mb-4">
            <div className="flex items-center gap-4">
              <div className="w-5 h-5 border-2 border-theme-border border-t-accent-link rounded-full animate-spin" />
              <span className="text-sm font-medium text-theme-text-primary">
                {trace.traceMode === 'perfetto' ? 'Opening in Perfetto...' : 'Downloading Trace...'}
              </span>
            </div>
            {trace.progress && (
              <p className="text-xs text-theme-text-secondary mt-2">
                {trace.progress.message}
              </p>
            )}
          </div>
        )}

        <PerformanceMetricsChart
          processId={processId}
          dataSource={defaultDataSource}
          selectedMeasure={selectedMeasure}
          measures={measures}
          discoveryDone={discoveryDone}
          discoveryLoading={discoveryLoading}
          noMeasuresAvailable={noMeasuresAvailable}
          binInterval={binInterval}
          apiTimeRange={apiTimeRange}
          scaleMode={scaleMode}
          selectedProperties={selectedProperties}
          queryError={queryError}
          setQueryError={setQueryError}
          onAddProperty={handleAddProperty}
          onRemoveProperty={handleRemoveProperty}
          onScaleModeChange={handleScaleModeChange}
          onTimeRangeSelect={handleTimeRangeSelect}
          onWidthChange={handleChartWidthChange}
          onAxisBoundsChange={handleAxisBoundsChange}
          customQueryRef={customQueryRef}
          onViewStateChange={onViewStateChange}
        />

        <ThreadCoveragePanel
          processId={processId}
          timeRange={apiTimeRange}
          dataSource={defaultDataSource}
          chartTimeRange={metricsView.chartTimeRange}
          chartAxisBounds={chartAxisBounds}
          onTimeRangeSelect={handleTimeRangeSelect}
          setTraceEventCount={setTraceEventCount}
          setTraceEventCountLoading={setTraceEventCountLoading}
          loadRef={loadThreadCoverageRef}
        />

        {metricsView.chartDataLength > 0 && (
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
