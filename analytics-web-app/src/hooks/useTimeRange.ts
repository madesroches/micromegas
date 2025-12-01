'use client'

import { useSearchParams, useRouter, usePathname } from 'next/navigation'
import { useCallback, useMemo } from 'react'
import {
  TimeRange,
  ParsedTimeRange,
  parseTimeRange,
  DEFAULT_TIME_RANGE,
  getTimeRangeForApi,
} from '@/lib/time-range'

export interface UseTimeRangeReturn {
  // Raw URL values
  timeRange: TimeRange
  // Parsed dates and label
  parsed: ParsedTimeRange
  // For API calls
  apiTimeRange: { begin: string; end: string }
  // Update functions
  setTimeRange: (from: string, to: string) => void
  setPreset: (preset: string) => void
}

export function useTimeRange(): UseTimeRangeReturn {
  const searchParams = useSearchParams()
  const router = useRouter()
  const pathname = usePathname()

  const timeRange: TimeRange = useMemo(() => {
    const from = searchParams.get('from') || DEFAULT_TIME_RANGE.from
    const to = searchParams.get('to') || DEFAULT_TIME_RANGE.to
    return { from, to }
  }, [searchParams])

  const parsed = useMemo(() => {
    try {
      return parseTimeRange(timeRange.from, timeRange.to)
    } catch {
      return parseTimeRange(DEFAULT_TIME_RANGE.from, DEFAULT_TIME_RANGE.to)
    }
  }, [timeRange])

  const apiTimeRange = useMemo(() => {
    try {
      return getTimeRangeForApi(timeRange.from, timeRange.to)
    } catch {
      return getTimeRangeForApi(DEFAULT_TIME_RANGE.from, DEFAULT_TIME_RANGE.to)
    }
  }, [timeRange])

  const setTimeRange = useCallback(
    (from: string, to: string) => {
      const params = new URLSearchParams(searchParams.toString())
      params.set('from', from)
      params.set('to', to)
      router.push(`${pathname}?${params.toString()}`)
    },
    [searchParams, router, pathname]
  )

  const setPreset = useCallback(
    (preset: string) => {
      setTimeRange(preset, 'now')
    },
    [setTimeRange]
  )

  return {
    timeRange,
    parsed,
    apiTimeRange,
    setTimeRange,
    setPreset,
  }
}
