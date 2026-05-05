import { useCallback, useMemo } from 'react'
import { useStreamQuery } from './useStreamQuery'
import { timestampToDate } from '@/lib/arrow-utils'
import type { MapEvent } from '@/components/map/MapViewer'

const DEFAULT_SQL = `SELECT
  time,
  process_id,
  properties->>'x' as x,
  properties->>'y' as y,
  properties->>'z' as z,
  properties->>'player_name' as player_name,
  properties->>'death_cause' as death_cause
FROM spans
WHERE name = 'death_event'
  AND time BETWEEN $begin AND $end
ORDER BY time DESC
LIMIT 10000`

export const MAP_VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
]

export interface UseMapDataReturn {
  events: MapEvent[]
  isLoading: boolean
  error: string | null
  execute: (sql?: string) => void
  currentSql: string
}

export interface UseMapDataParams {
  apiTimeRange: { begin: string; end: string }
}

export function useMapData({ apiTimeRange }: UseMapDataParams): UseMapDataReturn {
  const streamQuery = useStreamQuery()

  const execute = useCallback(
    (sql: string = DEFAULT_SQL) => {
      streamQuery.execute({
        sql,
        params: {
          begin: apiTimeRange.begin,
          end: apiTimeRange.end,
        },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    },
    [streamQuery, apiTimeRange]
  )

  const events = useMemo((): MapEvent[] => {
    const table = streamQuery.getTable()
    if (!table) return []

    const result: MapEvent[] = []
    for (let i = 0; i < table.numRows; i++) {
      const row = table.get(i)
      if (!row) continue

      const time = timestampToDate(row.time)
      const x = parseFloat(String(row.x ?? '0'))
      const y = parseFloat(String(row.y ?? '0'))
      const z = parseFloat(String(row.z ?? '0'))

      if (isNaN(x) || isNaN(y) || isNaN(z)) continue

      // Collect all extra columns as generic properties
      const properties: Record<string, string> = {}
      const skipKeys = new Set(['time', 'process_id', 'x', 'y', 'z'])
      const rowObj = row.toJSON?.() ?? row
      for (const [key, value] of Object.entries(rowObj)) {
        if (!skipKeys.has(key) && value != null && String(value).trim() !== '') {
          properties[key] = String(value)
        }
      }

      result.push({
        id: `${row.process_id}-${i}`,
        time: time ?? new Date(),
        processId: String(row.process_id ?? ''),
        x,
        y,
        z,
        properties,
      })
    }

    return result
  }, [streamQuery])

  return {
    events,
    isLoading: streamQuery.isStreaming,
    error: streamQuery.error?.message ?? null,
    execute,
    currentSql: DEFAULT_SQL,
  }
}
