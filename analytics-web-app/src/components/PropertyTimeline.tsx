import { useState, useCallback, useRef, useEffect } from 'react'
import { Plus, X } from 'lucide-react'
import { PropertyTimelineData } from '@/types'
import { ChartAxisBounds } from './TimeSeriesChart'

interface PropertyTimelineProps {
  properties: PropertyTimelineData[]
  availableKeys: string[]
  selectedKeys: string[]
  timeRange: { from: number; to: number }
  axisBounds?: ChartAxisBounds | null
  onTimeRangeSelect?: (from: Date, to: Date) => void
  onAddProperty: (key: string) => void
  onRemoveProperty: (key: string) => void
  isLoading?: boolean
}

export function PropertyTimeline({
  properties,
  availableKeys,
  selectedKeys,
  timeRange,
  axisBounds,
  onTimeRangeSelect,
  onAddProperty,
  onRemoveProperty,
  isLoading,
}: PropertyTimelineProps) {
  const duration = timeRange.to - timeRange.from
  const [selection, setSelection] = useState<{ startX: number; currentX: number } | null>(null)
  const [isDragging, setIsDragging] = useState(false)
  const [dropdownOpen, setDropdownOpen] = useState(false)
  const [tooltip, setTooltip] = useState<{
    visible: boolean
    x: number
    y: number
    propertyName: string
    value: string
    begin: Date
    end: Date
  } | null>(null)
  const dropdownRef = useRef<HTMLDivElement>(null)

  // Calculate position as percentage
  const toPercent = (time: number) => {
    return ((time - timeRange.from) / duration) * 100
  }

  // Clamp values to visible range
  const clamp = (value: number, min: number, max: number) => {
    return Math.max(min, Math.min(max, value))
  }

  // Use axis bounds if available, otherwise fall back to defaults
  const leftOffset = axisBounds ? axisBounds.left : 70 // Default Y-axis width
  const plotWidth = axisBounds ? axisBounds.width : undefined

  // Convert pixel position to time
  const pixelToTime = useCallback(
    (pixelX: number, containerWidth: number): number => {
      const ratio = pixelX / containerWidth
      return timeRange.from + ratio * duration
    },
    [timeRange.from, duration]
  )

  // Get pixel position relative to the timeline bar
  const getRelativeX = useCallback((e: React.MouseEvent, element: HTMLElement): number => {
    const rect = element.getBoundingClientRect()
    return Math.max(0, Math.min(e.clientX - rect.left, rect.width))
  }, [])

  const handleMouseDown = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!onTimeRangeSelect) return
      const target = e.currentTarget
      const x = getRelativeX(e, target)
      setSelection({ startX: x, currentX: x })
      setIsDragging(true)
      e.preventDefault()
    },
    [onTimeRangeSelect, getRelativeX]
  )

  const handleMouseMove = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!isDragging || !selection) return
      const target = e.currentTarget
      const x = getRelativeX(e, target)
      setSelection((prev) => (prev ? { ...prev, currentX: x } : null))
    },
    [isDragging, selection, getRelativeX]
  )

  const handleMouseUp = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!isDragging || !selection || !onTimeRangeSelect) {
        setSelection(null)
        setIsDragging(false)
        return
      }

      const target = e.currentTarget
      const containerWidth = target.getBoundingClientRect().width
      const minX = Math.min(selection.startX, selection.currentX)
      const maxX = Math.max(selection.startX, selection.currentX)

      // Only trigger if selection is meaningful (at least 5 pixels)
      if (maxX - minX > 5) {
        const fromTime = pixelToTime(minX, containerWidth)
        const toTime = pixelToTime(maxX, containerWidth)
        onTimeRangeSelect(new Date(fromTime), new Date(toTime))
      }

      setSelection(null)
      setIsDragging(false)
    },
    [isDragging, selection, onTimeRangeSelect, pixelToTime]
  )

  const handleMouseLeave = useCallback(() => {
    if (isDragging) {
      setSelection(null)
      setIsDragging(false)
    }
  }, [isDragging])

  // Calculate selection overlay position
  const getSelectionStyle = useCallback(() => {
    if (!selection) return { display: 'none' }
    const left = Math.min(selection.startX, selection.currentX)
    const width = Math.abs(selection.currentX - selection.startX)
    return {
      left: `${left}px`,
      width: `${width}px`,
      display: width > 2 ? 'block' : 'none',
    }
  }, [selection])

  // Close dropdown when clicking outside
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setDropdownOpen(false)
      }
    }
    document.addEventListener('mousedown', handleClickOutside)
    return () => document.removeEventListener('mousedown', handleClickOutside)
  }, [])

  // Available properties that haven't been selected
  const unselectedKeys = availableKeys.filter((key) => !selectedKeys.includes(key))

  const formatTime = (date: Date) => {
    return date.toLocaleTimeString('en-US', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hour12: false,
    })
  }

  return (
    <div className="bg-app-panel border border-theme-border rounded-lg">
      {/* Header */}
      <div className="px-4 py-3 border-b border-theme-border flex justify-between items-center">
        <div>
          <div className="text-base font-medium text-theme-text-primary">Properties</div>
          {isLoading && properties.length === 0 && selectedKeys.length > 0 && (
            <div className="text-xs text-theme-text-muted mt-1">Loading...</div>
          )}
        </div>

        {/* Add property dropdown */}
        <div className="relative" ref={dropdownRef}>
          <button
            onClick={() => setDropdownOpen(!dropdownOpen)}
            disabled={unselectedKeys.length === 0}
            className="flex items-center gap-1.5 px-3 py-1.5 bg-app-bg border border-theme-border rounded-md text-theme-text-secondary text-xs hover:border-theme-border-hover hover:text-theme-text-primary transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <Plus className="w-3.5 h-3.5" />
            Add property
          </button>

          {dropdownOpen && unselectedKeys.length > 0 && (
            <div className="absolute top-full right-0 mt-1 bg-app-bg border border-theme-border rounded-md min-w-[180px] max-h-[240px] overflow-y-auto z-50 shadow-lg">
              <div className="px-3 py-2 border-b border-theme-border text-[10px] text-theme-text-muted uppercase tracking-wide">
                Available Properties
              </div>
              {unselectedKeys.map((key) => (
                <button
                  key={key}
                  onClick={() => {
                    onAddProperty(key)
                    setDropdownOpen(false)
                  }}
                  className="w-full px-3 py-2 text-left text-xs text-theme-text-secondary hover:bg-white/5 hover:text-theme-text-primary transition-colors"
                >
                  {key}
                </button>
              ))}
            </div>
          )}
        </div>
      </div>

      {/* Timeline Content */}
      <div className="min-h-[60px]">
        {selectedKeys.length === 0 ? (
          /* Empty state */
          <div className="px-4 py-8 text-center text-sm text-theme-text-muted">
            <div className="text-theme-text-secondary mb-1">No properties selected</div>
            <div className="text-xs">Click &quot;Add property&quot; to show property values over time</div>
          </div>
        ) : (
          /* Property rows */
          <div className="p-4">
            <div className="divide-y divide-theme-border/50">
              {properties.map((property) => (
                <div key={property.propertyName} className="flex items-center h-9 group">
                  {/* Property label with remove button */}
                  <div
                    className="flex-shrink-0 flex items-center justify-between pr-2"
                    style={{ width: leftOffset }}
                  >
                    <span className="text-xs text-theme-text-secondary truncate" title={property.propertyName}>
                      {property.propertyName}
                    </span>
                    <button
                      onClick={() => onRemoveProperty(property.propertyName)}
                      className="opacity-0 group-hover:opacity-100 p-0.5 text-theme-text-muted hover:text-accent-error transition-all"
                      title="Remove property"
                    >
                      <X className="w-3 h-3" />
                    </button>
                  </div>

                  {/* Timeline bar area */}
                  <div
                    className={`h-7 relative bg-app-bg rounded flex items-center gap-0.5 px-1 ${onTimeRangeSelect ? 'cursor-crosshair' : ''}`}
                    style={{ width: plotWidth ?? '100%' }}
                    onMouseDown={handleMouseDown}
                    onMouseMove={handleMouseMove}
                    onMouseUp={handleMouseUp}
                    onMouseLeave={handleMouseLeave}
                  >
                    {/* Selection overlay */}
                    {selection && (
                      <div
                        className="absolute top-0 bottom-0 pointer-events-none z-20"
                        style={{
                          ...getSelectionStyle(),
                          background: 'var(--chart-selection)',
                          borderLeft: '2px solid var(--chart-selection-border)',
                          borderRight: '2px solid var(--chart-selection-border)',
                        }}
                      />
                    )}

                    {/* Property segments */}
                    {property.segments.map((segment, idx) => {
                      const startPercent = clamp(toPercent(segment.begin), 0, 100)
                      const endPercent = clamp(toPercent(segment.end), 0, 100)
                      const widthPercent = endPercent - startPercent

                      // Skip segments entirely outside the visible range
                      if (widthPercent <= 0) return null

                      return (
                        <div
                          key={idx}
                          className="absolute top-1 bottom-1 bg-brand-blue rounded-sm flex items-center justify-center text-[10px] font-medium text-white overflow-hidden transition-opacity hover:opacity-85 hover:ring-2 hover:ring-brand-gold"
                          style={{
                            left: `${startPercent}%`,
                            width: `${Math.max(widthPercent, 1)}%`,
                            minWidth: '20px',
                          }}
                          onMouseEnter={(e) => {
                            setTooltip({
                              visible: true,
                              x: e.clientX,
                              y: e.clientY,
                              propertyName: property.propertyName,
                              value: segment.value,
                              begin: new Date(segment.begin),
                              end: new Date(segment.end),
                            })
                          }}
                          onMouseMove={(e) => {
                            setTooltip((prev) =>
                              prev ? { ...prev, x: e.clientX, y: e.clientY } : null
                            )
                          }}
                          onMouseLeave={() => setTooltip(null)}
                        >
                          <span className="truncate px-1">{segment.value}</span>
                        </div>
                      )
                    })}
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* Tooltip */}
      {tooltip?.visible && (
        <div
          className="fixed bg-app-bg border border-theme-border rounded-md px-3 py-2 text-xs pointer-events-none z-50 shadow-lg"
          style={{
            left: tooltip.x + 15,
            top: tooltip.y - 10,
          }}
        >
          <div className="text-theme-text-muted text-[10px] mb-1">{tooltip.propertyName}</div>
          <div className="text-theme-text-primary font-medium">{tooltip.value}</div>
          <div className="text-theme-text-secondary text-[10px] mt-1">
            {formatTime(tooltip.begin)} → {formatTime(tooltip.end)}
          </div>
        </div>
      )}
    </div>
  )
}
