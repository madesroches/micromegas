import { useState, useEffect, useMemo, useCallback } from 'react'
import { useStreamQuery } from './useStreamQuery'
import { timestampToMs } from '@/lib/arrow-utils'
import { aggregateIntoSegments } from '@/lib/property-utils'
import { PropertyTimelineData } from '@/types'

const METRICS_SQL = `SELECT
  date_bin(INTERVAL '$bin_interval', time) as time,
  max(value) as value,
  jsonb_format_json(first_value(properties) FILTER (WHERE properties IS NOT NULL)) as properties
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
GROUP BY date_bin(INTERVAL '$bin_interval', time)
ORDER BY time`

interface UseMetricsDataParams {
  processId: string | null
  measureName: string | null
  binInterval: string
  apiTimeRange: { begin: string; end: string }
  enabled?: boolean
  dataSource?: string
}

interface UseMetricsDataReturn {
  chartData: { time: number; value: number }[]
  availablePropertyKeys: string[]
  getPropertyTimeline: (key: string) => PropertyTimelineData
  propertyParseErrors: string[]
  isLoading: boolean
  isComplete: boolean
  error: string | null
  execute: () => void
}

export function useMetricsData({
  processId,
  measureName,
  binInterval,
  apiTimeRange,
  enabled = true,
  dataSource,
}: UseMetricsDataParams): UseMetricsDataReturn {
  const query = useStreamQuery()

  const [chartData, setChartData] = useState<{ time: number; value: number }[]>([])
  const [rawPropertiesData, setRawPropertiesData] = useState<Map<number, Record<string, unknown>>>(new Map())
  const [propertyParseErrors, setPropertyParseErrors] = useState<string[]>([])

  // Execute the unified query
  const execute = useCallback(() => {
    if (!processId || !measureName || !enabled) return

    // Don't clear data immediately - keep showing existing data until new data arrives
    // This prevents the chart from flickering when window resizes (e.g., opening DevTools)

    // useStreamQuery handles cancellation internally via its own AbortController
    query.execute({
      sql: METRICS_SQL,
      params: {
        process_id: processId,
        measure_name: measureName,
        bin_interval: binInterval,
      },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
      dataSource,
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps -- Use stable refs (query.execute) and primitives
  }, [processId, measureName, binInterval, apiTimeRange.begin, apiTimeRange.end, enabled, dataSource, query.execute])

  // Cleanup: cancel query on unmount
  useEffect(() => {
    return () => {
      query.cancel()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- query.cancel is stable
  }, [query.cancel])

  // Extract data when query completes
  useEffect(() => {
    if (query.isComplete && !query.error) {
      const table = query.getTable()
      if (table) {
        const points: { time: number; value: number }[] = []
        const propsMap = new Map<number, Record<string, unknown>>()
        const errors: string[] = []

        for (let i = 0; i < table.numRows; i++) {
          const row = table.get(i)
          if (row) {
            const time = timestampToMs(row.time)
            points.push({ time, value: Number(row.value) })

            const propsStr = row.properties
            if (propsStr != null) {
              try {
                propsMap.set(time, JSON.parse(String(propsStr)))
              } catch (e) {
                errors.push(`Invalid JSON at time ${time}: ${e instanceof Error ? e.message : String(e)}`)
              }
            }
          }
        }

        setChartData(points)
        setRawPropertiesData(propsMap)
        setPropertyParseErrors(errors)
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- Only react to completion/error, not the full hook object
  }, [query.isComplete, query.error])

  // Derive available property keys from the data
  const availablePropertyKeys = useMemo(() => {
    const keysSet = new Set<string>()
    for (const props of rawPropertiesData.values()) {
      Object.keys(props).forEach(k => keysSet.add(k))
    }
    return Array.from(keysSet).sort()
  }, [rawPropertiesData])

  // Compute time range in milliseconds
  const timeRangeMs = useMemo(() => ({
    begin: new Date(apiTimeRange.begin).getTime(),
    end: new Date(apiTimeRange.end).getTime(),
  }), [apiTimeRange.begin, apiTimeRange.end])

  // Function to get property timeline for a specific key
  const getPropertyTimeline = useCallback((propertyName: string): PropertyTimelineData => {
    const rows: { time: number; value: string }[] = []

    const sortedEntries = Array.from(rawPropertiesData.entries()).sort((a, b) => a[0] - b[0])

    for (const [time, props] of sortedEntries) {
      const value = props[propertyName]
      if (value !== undefined && value !== null) {
        rows.push({ time, value: String(value) })
      }
    }

    return {
      propertyName,
      segments: aggregateIntoSegments(rows, timeRangeMs),
    }
  }, [rawPropertiesData, timeRangeMs])

  return {
    chartData,
    availablePropertyKeys,
    getPropertyTimeline,
    propertyParseErrors,
    isLoading: query.isStreaming,
    isComplete: query.isComplete,
    error: query.error?.message ?? null,
    execute,
  }
}
