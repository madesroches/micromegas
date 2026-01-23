import { useEffect, useRef } from 'react'
import { ScreenConfig } from '@/lib/screens-api'
import { DEFAULT_TIME_RANGE } from '@/lib/screen-defaults'

/** Config interface with time range properties */
interface TimeRangeConfig {
  timeRangeFrom?: string
  timeRangeTo?: string
  [key: string]: unknown
}

interface TimeRangeSyncParams {
  /** Current raw time range from URL */
  rawTimeRange: { from: string; to: string }
  /** Saved config (null for new screens) */
  savedConfig: TimeRangeConfig | null
  /** Current working config */
  config: TimeRangeConfig
  /** Set unsaved changes state */
  setHasUnsavedChanges: (value: boolean) => void
  /** Callback to update config */
  onConfigChange: (config: ScreenConfig) => void
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
export function useTimeRangeSync({
  rawTimeRange,
  savedConfig,
  config,
  setHasUnsavedChanges,
  onConfigChange,
}: TimeRangeSyncParams): void {
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
    const savedFrom = savedConfig?.timeRangeFrom ?? DEFAULT_TIME_RANGE.from
    const savedTo = savedConfig?.timeRangeTo ?? DEFAULT_TIME_RANGE.to
    setHasUnsavedChanges(current.from !== savedFrom || current.to !== savedTo)

    // Update config with time range
    onConfigChange({
      ...config,
      timeRangeFrom: current.from,
      timeRangeTo: current.to,
    })
  }, [rawTimeRange, savedConfig, config, setHasUnsavedChanges, onConfigChange])
}
