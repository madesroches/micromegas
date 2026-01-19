import { useSearchParams, useNavigate, useLocation } from 'react-router-dom'
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
}

export function useTimeRange(): UseTimeRangeReturn {
  const [searchParams] = useSearchParams()
  const navigate = useNavigate()
  const location = useLocation()
  const pathname = location.pathname

  // Extract actual string values from searchParams to avoid reference instability
  const fromParam = searchParams.get('from') || DEFAULT_TIME_RANGE.from
  const toParam = searchParams.get('to') || DEFAULT_TIME_RANGE.to

  const timeRange: TimeRange = useMemo(() => {
    return { from: fromParam, to: toParam }
  }, [fromParam, toParam])

  const parsed = useMemo(() => {
    try {
      return parseTimeRange(fromParam, toParam)
    } catch {
      return parseTimeRange(DEFAULT_TIME_RANGE.from, DEFAULT_TIME_RANGE.to)
    }
  }, [fromParam, toParam])

  // Memoize API time range - only recalculates when from/to params change
  const apiTimeRange = useMemo(() => {
    try {
      return getTimeRangeForApi(fromParam, toParam)
    } catch {
      return getTimeRangeForApi(DEFAULT_TIME_RANGE.from, DEFAULT_TIME_RANGE.to)
    }
  }, [fromParam, toParam])

  // Use location.search (a string) instead of searchParams object
  // to avoid callback instability - searchParams is a new object every render
  const setTimeRange = useCallback(
    (from: string, to: string) => {
      const params = new URLSearchParams(location.search)
      params.set('from', from)
      params.set('to', to)
      navigate(`${pathname}?${params.toString()}`)
    },
    [location.search, navigate, pathname]
  )

  return {
    timeRange,
    parsed,
    apiTimeRange,
    setTimeRange,
  }
}
