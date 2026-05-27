/**
 * Metrics chart section for the Performance Analysis page.
 *
 * Owns the unified metrics query (via useMetricsData), the custom-query path,
 * and the metrics-execute effect, and renders the chart area (chart / loading /
 * empty states). It exposes run/reset for the page's SQL editor through a ref
 * and lifts the gate-relevant view-state (hasLoaded, loading, chart time range,
 * parse errors) to the page so the page keeps owning the re-fetch gate (#1089).
 */
import { useCallback, useEffect, useMemo, useRef, useState, type MutableRefObject } from 'react'
import { Clock } from 'lucide-react'
import { MetricsChart, ScaleMode } from '@/components/MetricsChart'
import { executeStreamQuery } from '@/lib/arrow-stream'
import { timestampToMs } from '@/lib/arrow-utils'
import {
  createPropertyTimelineGetter,
  extractPropertiesFromRows,
  ExtractedPropertyData,
} from '@/lib/property-utils'
import { useMetricsData } from '@/hooks/useMetricsData'
import type { Measure } from './queries'

export interface MetricsViewState {
  hasLoaded: boolean
  isLoading: boolean
  chartTimeRange: { from: number; to: number } | null
  chartDataLength: number
  propertyParseErrors: string[]
}

export interface CustomQueryHandle {
  run: (sql: string) => void
  reset: () => void
}

interface PerformanceMetricsChartProps {
  processId: string
  dataSource?: string
  selectedMeasure: string | null
  measures: Measure[]
  discoveryDone: boolean
  discoveryLoading: boolean
  noMeasuresAvailable: boolean
  binInterval: string
  apiTimeRange: { begin: string; end: string }
  scaleMode: ScaleMode
  selectedProperties: string[]
  queryError: string | null
  setQueryError: (message: string | null) => void
  onAddProperty: (key: string) => void
  onRemoveProperty: (key: string) => void
  onScaleModeChange: (mode: ScaleMode) => void
  onTimeRangeSelect: (from: Date, to: Date) => void
  onWidthChange: (width: number) => void
  onAxisBoundsChange: (bounds: import('@/components/XYChart').ChartAxisBounds) => void
  /** Page registers run/reset so the SQL editor and measure change can drive custom queries. */
  customQueryRef: MutableRefObject<CustomQueryHandle | null>
  onViewStateChange: (state: MetricsViewState) => void
}

export function PerformanceMetricsChart({
  processId,
  dataSource,
  selectedMeasure,
  measures,
  discoveryDone,
  discoveryLoading,
  noMeasuresAvailable,
  binInterval,
  apiTimeRange,
  scaleMode,
  selectedProperties,
  queryError,
  setQueryError,
  onAddProperty,
  onRemoveProperty,
  onScaleModeChange,
  onTimeRangeSelect,
  onWidthChange,
  onAxisBoundsChange,
  customQueryRef,
  onViewStateChange,
}: PerformanceMetricsChartProps) {
  const [isCustomQuery, setIsCustomQuery] = useState(false)
  const [customQueryLoading, setCustomQueryLoading] = useState(false)
  const [customChartData, setCustomChartData] = useState<{ time: number; value: number }[]>([])
  const [customPropertyData, setCustomPropertyData] = useState<ExtractedPropertyData>({
    availableKeys: [],
    rawData: new Map(),
    errors: [],
  })

  // Unified metrics data hook (Model layer)
  const metricsData = useMetricsData({
    processId,
    measureName: selectedMeasure,
    binInterval,
    apiTimeRange,
    enabled: !!processId && !!selectedMeasure,
    dataSource,
  })

  // Use unified data or custom query data
  const chartData = isCustomQuery ? customChartData : metricsData.chartData
  const dataLoading = isCustomQuery ? false : metricsData.isLoading
  const hasLoaded = isCustomQuery
    ? customChartData.length > 0 || queryError !== null
    : metricsData.isComplete

  // Compute time range in milliseconds for property timeline
  const timeRangeMs = useMemo(
    () => ({
      begin: new Date(apiTimeRange.begin).getTime(),
      end: new Date(apiTimeRange.end).getTime(),
    }),
    [apiTimeRange.begin, apiTimeRange.end]
  )

  // Use custom or unified property data based on query mode
  const availablePropertyKeys = isCustomQuery
    ? customPropertyData.availableKeys
    : metricsData.availablePropertyKeys
  const getPropertyTimeline = useMemo(
    () =>
      isCustomQuery
        ? createPropertyTimelineGetter(customPropertyData.rawData, timeRangeMs)
        : metricsData.getPropertyTimeline,
    [isCustomQuery, customPropertyData.rawData, timeRangeMs, metricsData.getPropertyTimeline]
  )
  const propertyParseErrors = isCustomQuery
    ? customPropertyData.errors
    : metricsData.propertyParseErrors

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

  const loadCustomQuery = useCallback(
    async (sql: string) => {
      if (!processId || !selectedMeasure) return
      setQueryError(null)
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
          dataSource,
        })

        if (error) {
          setQueryError(error.message)
          setCustomQueryLoading(false)
          return
        }

        const points: { time: number; value: number }[] = []
        const propsRows: { time: number; properties: string | null }[] = []
        // Check if any batch has a properties column
        const hasPropertiesColumn =
          batches.length > 0 && batches[0].schema.fields.some((f) => f.name === 'properties')

        for (const batch of batches) {
          for (let i = 0; i < batch.numRows; i++) {
            const row = batch.get(i)
            if (row) {
              const time = timestampToMs(row.time)
              points.push({ time, value: Number(row.value) })
              if (hasPropertiesColumn) {
                propsRows.push({ time, properties: row.properties != null ? String(row.properties) : null })
              }
            }
          }
        }

        setCustomChartData(points)
        setCustomPropertyData(
          hasPropertiesColumn
            ? extractPropertiesFromRows(propsRows)
            : { availableKeys: [], rawData: new Map(), errors: [] }
        )
      } catch (err) {
        setQueryError(err instanceof Error ? err.message : 'Unknown error')
      } finally {
        setCustomQueryLoading(false)
      }
    },
    [processId, selectedMeasure, binInterval, apiTimeRange.begin, apiTimeRange.end, dataSource, setQueryError]
  )

  // Expose run/reset to the page (SQL editor + measure change).
  customQueryRef.current = {
    run: loadCustomQuery,
    reset: () => setIsCustomQuery(false),
  }

  // Trigger unified query when discovery is done and measure is selected.
  // Use a ref so the effect doesn't re-run when execute's identity changes.
  const metricsDataExecuteRef = useRef(metricsData.execute)
  metricsDataExecuteRef.current = metricsData.execute
  useEffect(() => {
    if (discoveryDone && selectedMeasure && processId && !isCustomQuery) {
      metricsDataExecuteRef.current()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- Use primitive deps to avoid object comparison issues
  }, [discoveryDone, selectedMeasure, processId, isCustomQuery, binInterval, apiTimeRange.begin, apiTimeRange.end])

  // Lift gate-relevant view state to the page.
  const isLoading = dataLoading || customQueryLoading
  useEffect(() => {
    onViewStateChange({
      hasLoaded,
      isLoading,
      chartTimeRange,
      chartDataLength: chartData.length,
      propertyParseErrors,
    })
  }, [hasLoaded, isLoading, chartTimeRange, chartData.length, propertyParseErrors, onViewStateChange])

  // Show loading when discovery is done, measure selected, but data hasn't loaded yet
  const showDataLoading =
    isLoading || (discoveryDone && !!selectedMeasure && !hasLoaded && chartData.length === 0)
  const noDataInRange = hasLoaded && chartData.length === 0 && !!selectedMeasure

  return (
    <div className="mb-4">
      {selectedMeasure && chartData.length > 0 ? (
        <MetricsChart
          data={chartData}
          title={selectedMeasure}
          unit={selectedMeasureInfo?.unit || ''}
          availablePropertyKeys={availablePropertyKeys}
          getPropertyTimeline={getPropertyTimeline}
          selectedProperties={selectedProperties}
          onAddProperty={onAddProperty}
          onRemoveProperty={onRemoveProperty}
          scaleMode={scaleMode}
          onScaleModeChange={onScaleModeChange}
          onTimeRangeSelect={onTimeRangeSelect}
          onWidthChange={onWidthChange}
          onAxisBoundsChange={onAxisBoundsChange}
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
  )
}
