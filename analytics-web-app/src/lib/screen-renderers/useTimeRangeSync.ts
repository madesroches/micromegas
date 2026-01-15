import { useEffect, useRef } from 'react'
import { ScreenConfig } from '@/lib/screens-api'

const DEFAULT_TIME_RANGE_FROM = 'now-5m'
const DEFAULT_TIME_RANGE_TO = 'now'

interface TimeRangeSyncParams<T extends ScreenConfig> {
  /** Current raw time range from URL */
  rawTimeRange: { from: string; to: string }
  /** Saved config (null for new screens) */
  savedConfig: T | null
  /** Current working config */
  config: T
  /** Callback when there are unsaved changes */
  onUnsavedChange: () => void
  /** Callback to update config */
  onConfigChange: (config: T) => void
}

/**
 * Hook to sync time range changes to screen config.
 *
 * Handles:
 * - Detecting time range changes
 * - Marking unsaved changes when time range differs from saved config
 * - Updating config with new time range values
 *
 * This eliminates ~30 lines of duplicated code from each renderer.
 */
export function useTimeRangeSync<T extends ScreenConfig>({
  rawTimeRange,
  savedConfig,
  config,
  onUnsavedChange,
  onConfigChange,
}: TimeRangeSyncParams<T>): void {
  const prevTimeRangeRef = useRef<{ from: string; to: string } | null>(null)

  useEffect(() => {
    const current = { from: rawTimeRange.from, to: rawTimeRange.to }

    // On first run, just store current values
    if (prevTimeRangeRef.current === null) {
      prevTimeRangeRef.current = current
      return
    }

    const prev = prevTimeRangeRef.current
    if (prev.from === current.from && prev.to === current.to) {
      return
    }

    prevTimeRangeRef.current = current

    // Check if differs from saved config
    const savedFrom = savedConfig?.timeRangeFrom ?? DEFAULT_TIME_RANGE_FROM
    const savedTo = savedConfig?.timeRangeTo ?? DEFAULT_TIME_RANGE_TO
    if (current.from !== savedFrom || current.to !== savedTo) {
      onUnsavedChange()
    }

    // Update config with time range
    onConfigChange({
      ...config,
      timeRangeFrom: current.from,
      timeRangeTo: current.to,
    })
  }, [rawTimeRange, savedConfig, config, onUnsavedChange, onConfigChange])
}
