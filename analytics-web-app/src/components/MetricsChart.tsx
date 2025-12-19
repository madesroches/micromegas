import { useState, useCallback } from 'react'
import { TimeSeriesChart, ChartAxisBounds } from './TimeSeriesChart'
import { PropertyTimeline } from './PropertyTimeline'
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
  onTimeRangeSelect,
  onWidthChange,
  onAxisBoundsChange,
}: MetricsChartProps) {
  const [axisBounds, setAxisBounds] = useState<ChartAxisBounds | null>(null)
  const [selectedProperties, setSelectedProperties] = useState<string[]>([])

  // Fetch available property keys
  const {
    keys: availableKeys,
    isLoading: keysLoading,
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
  } = usePropertyTimeline({
    processId,
    measureName,
    propertyNames: selectedProperties,
    apiTimeRange,
    binInterval,
    enabled: !!processId && !!measureName && selectedProperties.length > 0,
  })

  const handleAxisBoundsChange = useCallback((bounds: ChartAxisBounds) => {
    setAxisBounds(bounds)
    onAxisBoundsChange?.(bounds)
  }, [onAxisBoundsChange])

  const handleAddProperty = useCallback((key: string) => {
    setSelectedProperties((prev) => [...prev, key])
  }, [])

  const handleRemoveProperty = useCallback((key: string) => {
    setSelectedProperties((prev) => prev.filter((k) => k !== key))
  }, [])

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

      {/* Property Timeline */}
      {showPropertyTimeline && (
        <PropertyTimeline
          properties={propertyTimelines}
          availableKeys={availableKeys}
          selectedKeys={selectedProperties}
          timeRange={chartTimeRange}
          axisBounds={axisBounds}
          onTimeRangeSelect={onTimeRangeSelect}
          onAddProperty={handleAddProperty}
          onRemoveProperty={handleRemoveProperty}
          isLoading={keysLoading || timelinesLoading}
        />
      )}

    </div>
  )
}
