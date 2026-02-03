import { useCallback, useMemo } from 'react'
import { useStreamQuery } from './useStreamQuery'
import { timestampToDate } from '@/lib/arrow-utils'
import type { DeathEvent } from '@/components/map/MapViewer'

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
  events: DeathEvent[]
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

  const events = useMemo((): DeathEvent[] => {
    const table = streamQuery.getTable()
    if (!table) return []

    const result: DeathEvent[] = []
    for (let i = 0; i < table.numRows; i++) {
      const row = table.get(i)
      if (!row) continue

      const time = timestampToDate(row.time)
      const x = parseFloat(String(row.x ?? '0'))
      const y = parseFloat(String(row.y ?? '0'))
      const z = parseFloat(String(row.z ?? '0'))

      if (isNaN(x) || isNaN(y) || isNaN(z)) continue

      result.push({
        id: `${row.process_id}-${i}`,
        time: time ?? new Date(),
        processId: String(row.process_id ?? ''),
        x,
        y,
        z,
        playerName: row.player_name ? String(row.player_name) : undefined,
        deathCause: row.death_cause ? String(row.death_cause) : undefined,
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
