import { useState, useEffect, useRef, useCallback } from 'react'
import { useMutation } from '@tanstack/react-query'
import { executeSqlQuery, toRowObjects } from '@/lib/api'
import { PropertyTimelineData, PropertySegment } from '@/types'

const PROPERTY_VALUES_SQL = `SELECT
  date_bin(INTERVAL '$bin_interval', time) as time,
  first_value(property_get(properties, '$property_name')) as value
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
  AND properties IS NOT NULL
GROUP BY date_bin(INTERVAL '$bin_interval', time)
ORDER BY time`

interface UsePropertyTimelineParams {
  processId: string | null
  measureName: string | null
  propertyNames: string[]
  apiTimeRange: { begin: string; end: string }
  binInterval: string
  enabled?: boolean
}

interface UsePropertyTimelineReturn {
  timelines: PropertyTimelineData[]
  isLoading: boolean
  error: string | null
  refetch: () => void
}

interface RawPropertyData {
  propertyName: string
  rows: { time: number; value: string }[]
}

function parseIntervalToMs(interval: string): number {
  const match = interval.match(/^(\d+)\s*(second|minute|hour|day)s?$/i)
  if (!match) return 60000 // default to 1 minute

  const value = parseInt(match[1], 10)
  const unit = match[2].toLowerCase()

  switch (unit) {
    case 'second':
      return value * 1000
    case 'minute':
      return value * 60 * 1000
    case 'hour':
      return value * 60 * 60 * 1000
    case 'day':
      return value * 24 * 60 * 60 * 1000
    default:
      return 60000
  }
}

function aggregateIntoSegments(
  rows: { time: number; value: string }[],
  binIntervalMs: number
): PropertySegment[] {
  if (rows.length === 0) return []

  const segments: PropertySegment[] = []
  let currentSegment: PropertySegment | null = null

  for (const row of rows) {
    const binEnd = row.time + binIntervalMs

    if (!currentSegment) {
      currentSegment = {
        value: row.value,
        begin: row.time,
        end: binEnd,
      }
    } else if (currentSegment.value === row.value) {
      // Extend current segment to cover this bin
      currentSegment.end = binEnd
    } else {
      // Close current segment and start new one
      segments.push(currentSegment)
      currentSegment = {
        value: row.value,
        begin: row.time,
        end: binEnd,
      }
    }
  }

  // Don't forget the last segment
  if (currentSegment) {
    segments.push(currentSegment)
  }

  return segments
}

export function usePropertyTimeline({
  processId,
  measureName,
  propertyNames,
  apiTimeRange,
  binInterval,
  enabled = true,
}: UsePropertyTimelineParams): UsePropertyTimelineReturn {
  const [timelines, setTimelines] = useState<PropertyTimelineData[]>([])
  const [error, setError] = useState<string | null>(null)
  const [pendingRequests, setPendingRequests] = useState<Set<string>>(new Set())

  const rawDataRef = useRef<Map<string, RawPropertyData>>(new Map())
  const pendingRequestsRef = useRef<Set<string>>(new Set())

  const mutation = useMutation({
    mutationFn: async ({
      sql,
      params,
      begin,
      end,
      propertyName,
    }: {
      sql: string
      params: Record<string, string>
      begin: string
      end: string
      propertyName: string
    }) => {
      const result = await executeSqlQuery({ sql, params, begin, end })
      return { result, propertyName }
    },
    onSuccess: ({ result, propertyName }) => {
      const rows = toRowObjects(result)
      const parsedRows = rows.map((row) => ({
        time: new Date(String(row.time)).getTime(),
        value: String(row.value ?? ''),
      }))

      rawDataRef.current.set(propertyName, {
        propertyName,
        rows: parsedRows,
      })

      pendingRequestsRef.current.delete(propertyName)
      setPendingRequests(new Set(pendingRequestsRef.current))

      // Update timelines when all requests are complete
      updateTimelines()
    },
    onError: (err: Error, { propertyName }) => {
      setError(err.message)
      pendingRequestsRef.current.delete(propertyName)
      setPendingRequests(new Set(pendingRequestsRef.current))
    },
  })

  const mutateRef = useRef(mutation.mutate)
  mutateRef.current = mutation.mutate

  const updateTimelines = useCallback(() => {
    const newTimelines: PropertyTimelineData[] = []
    const binIntervalMs = parseIntervalToMs(binInterval)

    for (const propertyName of propertyNames) {
      const data = rawDataRef.current.get(propertyName)
      if (data) {
        newTimelines.push({
          propertyName: data.propertyName,
          segments: aggregateIntoSegments(data.rows, binIntervalMs),
        })
      }
    }

    setTimelines(newTimelines)
  }, [propertyNames, binInterval])

  const fetchProperty = useCallback(
    (propertyName: string) => {
      if (!processId || !measureName || !enabled) return
      if (pendingRequestsRef.current.has(propertyName)) return

      pendingRequestsRef.current.add(propertyName)
      setPendingRequests(new Set(pendingRequestsRef.current))

      mutateRef.current({
        sql: PROPERTY_VALUES_SQL,
        params: {
          process_id: processId,
          measure_name: measureName,
          property_name: propertyName,
          bin_interval: binInterval,
        },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
        propertyName,
      })
    },
    [processId, measureName, apiTimeRange.begin, apiTimeRange.end, binInterval, enabled]
  )

  const fetchAllProperties = useCallback(() => {
    if (!enabled || !processId || !measureName) return

    // Clear old data for properties no longer selected
    for (const key of rawDataRef.current.keys()) {
      if (!propertyNames.includes(key)) {
        rawDataRef.current.delete(key)
      }
    }

    // Fetch data for all selected properties
    for (const propertyName of propertyNames) {
      fetchProperty(propertyName)
    }

    // If no properties, clear timelines
    if (propertyNames.length === 0) {
      setTimelines([])
    }
  }, [processId, measureName, propertyNames, enabled, fetchProperty])

  // Track previous params to detect changes
  const prevParamsRef = useRef<{
    processId: string | null
    measureName: string | null
    propertyNames: string[]
    begin: string
    end: string
    binInterval: string
  } | null>(null)

  useEffect(() => {
    if (!enabled || !processId || !measureName) {
      return
    }

    const currentParams = {
      processId,
      measureName,
      propertyNames: [...propertyNames].sort(),
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
      binInterval,
    }

    // Check if params changed (other than propertyNames)
    const baseParamsChanged =
      !prevParamsRef.current ||
      prevParamsRef.current.processId !== currentParams.processId ||
      prevParamsRef.current.measureName !== currentParams.measureName ||
      prevParamsRef.current.begin !== currentParams.begin ||
      prevParamsRef.current.end !== currentParams.end ||
      prevParamsRef.current.binInterval !== currentParams.binInterval

    // Check if propertyNames changed
    const propertyNamesChanged =
      !prevParamsRef.current ||
      prevParamsRef.current.propertyNames.join(',') !== currentParams.propertyNames.join(',')

    if (baseParamsChanged) {
      // Clear all cached data and refetch everything
      rawDataRef.current.clear()
      pendingRequestsRef.current.clear()
      setPendingRequests(new Set())
      prevParamsRef.current = currentParams
      fetchAllProperties()
    } else if (propertyNamesChanged) {
      // Only fetch new properties
      const prevNames = prevParamsRef.current?.propertyNames ?? []
      const addedNames = propertyNames.filter((name) => !prevNames.includes(name))
      const removedNames = prevNames.filter((name) => !propertyNames.includes(name))

      // Remove data for removed properties
      for (const name of removedNames) {
        rawDataRef.current.delete(name)
      }

      // Fetch data for added properties
      for (const name of addedNames) {
        fetchProperty(name)
      }

      // Update timelines immediately for removals
      if (removedNames.length > 0 && addedNames.length === 0) {
        updateTimelines()
      }

      prevParamsRef.current = currentParams
    }
  }, [
    processId,
    measureName,
    propertyNames,
    apiTimeRange.begin,
    apiTimeRange.end,
    binInterval,
    enabled,
    fetchAllProperties,
    fetchProperty,
    updateTimelines,
  ])

  return {
    timelines,
    isLoading: pendingRequests.size > 0 || mutation.isPending,
    error,
    refetch: fetchAllProperties,
  }
}
