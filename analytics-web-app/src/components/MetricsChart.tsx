import { useState, useCallback, useMemo } from 'react'
import { TimeSeriesChart, ChartAxisBounds, ScaleMode } from './XYChart'
import { PropertyTimeline } from './PropertyTimeline'
import { PropertyTimelineData } from '@/types'

interface MetricsChartProps {
  // Chart data
  data: { time: number; value: number }[]
  title: string
  unit: string
  // Property data (from unified query via Model layer)
  availablePropertyKeys: string[]
  getPropertyTimeline: (key: string) => PropertyTimelineData
  // Selected properties (controlled from parent for URL persistence)
  selectedProperties: string[]
  onAddProperty: (key: string) => void
  onRemoveProperty: (key: string) => void
  // Scale mode (controlled from parent for URL persistence)
  scaleMode?: ScaleMode
  onScaleModeChange?: (mode: ScaleMode) => void
  // Callbacks
  onTimeRangeSelect?: (from: Date, to: Date) => void
  onWidthChange?: (width: number) => void
  onAxisBoundsChange?: (bounds: ChartAxisBounds) => void
}

export type { ScaleMode }

export function MetricsChart({
  data,
  title,
  unit,
  availablePropertyKeys,
  getPropertyTimeline,
  selectedProperties,
  onAddProperty,
  onRemoveProperty,
  scaleMode,
  onScaleModeChange,
  onTimeRangeSelect,
  onWidthChange,
  onAxisBoundsChange,
}: MetricsChartProps) {
  const [axisBounds, setAxisBounds] = useState<ChartAxisBounds | null>(null)

  const handleAxisBoundsChange = useCallback((bounds: ChartAxisBounds) => {
    setAxisBounds(bounds)
    onAxisBoundsChange?.(bounds)
  }, [onAxisBoundsChange])

  // Derive property timelines from the getPropertyTimeline function
  const propertyTimelines = useMemo(() => {
    return selectedProperties.map(key => getPropertyTimeline(key))
  }, [selectedProperties, getPropertyTimeline])

  // Calculate time range from data for property timeline
  const chartTimeRange =
    data.length > 0
      ? {
          from: Math.min(...data.map((d) => d.time)),
          to: Math.max(...data.map((d) => d.time)),
        }
      : null

  const showPropertyTimeline = availablePropertyKeys.length > 0 && chartTimeRange

  return (
    <div className="flex flex-col gap-4">
      {/* Time Series Chart */}
      <div className="h-[350px]">
        <TimeSeriesChart
          data={data}
          title={title}
          unit={unit}
          scaleMode={scaleMode}
          onScaleModeChange={onScaleModeChange}
          onTimeRangeSelect={onTimeRangeSelect}
          onWidthChange={onWidthChange}
          onAxisBoundsChange={handleAxisBoundsChange}
        />
      </div>

      {/* Property Timeline */}
      {showPropertyTimeline && (
        <PropertyTimeline
          properties={propertyTimelines}
          availableKeys={availablePropertyKeys}
          selectedKeys={selectedProperties}
          timeRange={chartTimeRange}
          axisBounds={axisBounds}
          onTimeRangeSelect={onTimeRangeSelect}
          onAddProperty={onAddProperty}
          onRemoveProperty={onRemoveProperty}
        />
      )}

    </div>
  )
}
