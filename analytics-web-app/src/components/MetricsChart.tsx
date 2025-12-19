import { useState, useCallback } from 'react'
import { TimeSeriesChart, ChartAxisBounds } from './TimeSeriesChart'
import { PropertyTimeline } from './PropertyTimeline'
import { ErrorBanner } from './ErrorBanner'
import { usePropertyKeys } from '@/hooks/usePropertyKeys'
import { usePropertyTimeline } from '@/hooks/usePropertyTimeline'

interface MetricsChartProps {
  // Chart data
  data: { time: number; value: number }[]
  title: string
  unit: string
  // For property timeline
  processId: string | null
  measureName: string | null
  apiTimeRange: { begin: string; end: string }
  binInterval: string
  // Selected properties (controlled from parent for URL persistence)
  selectedProperties: string[]
  onAddProperty: (key: string) => void
  onRemoveProperty: (key: string) => void
  // Callbacks
  onTimeRangeSelect?: (from: Date, to: Date) => void
  onWidthChange?: (width: number) => void
  onAxisBoundsChange?: (bounds: ChartAxisBounds) => void
}

export function MetricsChart({
  data,
  title,
  unit,
  processId,
  measureName,
  apiTimeRange,
  binInterval,
  selectedProperties,
  onAddProperty,
  onRemoveProperty,
  onTimeRangeSelect,
  onWidthChange,
  onAxisBoundsChange,
}: MetricsChartProps) {
  const [axisBounds, setAxisBounds] = useState<ChartAxisBounds | null>(null)

  // Fetch available property keys
  const {
    keys: availableKeys,
    isLoading: keysLoading,
    error: keysError,
    refetch: refetchKeys,
  } = usePropertyKeys({
    processId,
    measureName,
    apiTimeRange,
    enabled: !!processId && !!measureName,
  })

  // Fetch property timeline data
  const {
    timelines: propertyTimelines,
    isLoading: timelinesLoading,
    error: timelinesError,
    refetch: refetchTimelines,
  } = usePropertyTimeline({
    processId,
    measureName,
    propertyNames: selectedProperties,
    apiTimeRange,
    binInterval,
    enabled: !!processId && !!measureName && selectedProperties.length > 0,
  })

  // Combine errors for display
  const propertyError = keysError || timelinesError
  const handleRetry = useCallback(() => {
    if (keysError) refetchKeys()
    if (timelinesError) refetchTimelines()
  }, [keysError, timelinesError, refetchKeys, refetchTimelines])

  const handleAxisBoundsChange = useCallback((bounds: ChartAxisBounds) => {
    setAxisBounds(bounds)
    onAxisBoundsChange?.(bounds)
  }, [onAxisBoundsChange])

  // Calculate time range from data for property timeline
  const chartTimeRange =
    data.length > 0
      ? {
          from: Math.min(...data.map((d) => d.time)),
          to: Math.max(...data.map((d) => d.time)),
        }
      : null

  const showPropertyTimeline = availableKeys.length > 0 && chartTimeRange

  return (
    <div className="flex flex-col gap-4">
      {/* Time Series Chart */}
      <div className="h-[350px] overflow-hidden">
        <TimeSeriesChart
          data={data}
          title={title}
          unit={unit}
          onTimeRangeSelect={onTimeRangeSelect}
          onWidthChange={onWidthChange}
          onAxisBoundsChange={handleAxisBoundsChange}
        />
      </div>

      {/* Property Error */}
      {propertyError && (
        <ErrorBanner
          title="Failed to load properties"
          message={propertyError}
          variant="warning"
          onRetry={handleRetry}
        />
      )}

      {/* Property Timeline */}
      {showPropertyTimeline && (
        <PropertyTimeline
          properties={propertyTimelines}
          availableKeys={availableKeys}
          selectedKeys={selectedProperties}
          timeRange={chartTimeRange}
          axisBounds={axisBounds}
          onTimeRangeSelect={onTimeRangeSelect}
          onAddProperty={onAddProperty}
          onRemoveProperty={onRemoveProperty}
          isLoading={keysLoading || timelinesLoading}
        />
      )}

    </div>
  )
}
