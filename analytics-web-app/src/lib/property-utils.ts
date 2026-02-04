import { PropertySegment, PropertyTimelineData } from '@/types'

/**
 * Result of extracting properties from query results.
 */
export interface ExtractedPropertyData {
  availableKeys: string[]
  rawData: Map<number, Record<string, unknown>>
  errors: string[]
}

/**
 * Extract property data from query result rows.
 * Returns available keys, raw property data map, and any parse errors.
 */
export function extractPropertiesFromRows(
  rows: { time: number; properties: string | null }[]
): ExtractedPropertyData {
  const rawData = new Map<number, Record<string, unknown>>()
  const keysSet = new Set<string>()
  const errors: string[] = []

  for (const row of rows) {
    if (row.properties != null) {
      try {
        const props = JSON.parse(row.properties)
        rawData.set(row.time, props)
        Object.keys(props).forEach(k => keysSet.add(k))
      } catch (e) {
        errors.push(`Invalid JSON at time ${row.time}: ${e instanceof Error ? e.message : String(e)}`)
      }
    }
  }

  return {
    availableKeys: Array.from(keysSet).sort(),
    rawData,
    errors,
  }
}

/**
 * Create a function that returns property timeline data for a given key.
 */
export function createPropertyTimelineGetter(
  rawData: Map<number, Record<string, unknown>>,
  timeRange?: { begin: number; end: number }
): (propertyName: string) => PropertyTimelineData {
  return (propertyName: string): PropertyTimelineData => {
    const rows: { time: number; value: string }[] = []
    const sortedEntries = Array.from(rawData.entries()).sort((a, b) => a[0] - b[0])

    for (const [time, props] of sortedEntries) {
      const value = props[propertyName]
      if (value !== undefined && value !== null) {
        rows.push({ time, value: String(value) })
      }
    }

    return {
      propertyName,
      segments: aggregateIntoSegments(rows, timeRange),
    }
  }
}

/**
 * Aggregate time-value rows into contiguous segments where adjacent rows
 * with the same value are merged. Segment boundaries are derived from the data itself.
 */
export function aggregateIntoSegments(
  rows: { time: number; value: string }[],
  timeRange?: { begin: number; end: number }
): PropertySegment[] {
  if (rows.length === 0) return []

  const segments: PropertySegment[] = []
  let currentSegment: PropertySegment | null = null

  for (let i = 0; i < rows.length; i++) {
    const row = rows[i]
    const nextTime = rows[i + 1]?.time

    if (!currentSegment) {
      // First segment starts at actual data point (not timeRange.begin)
      // to align with chart rendering
      currentSegment = {
        value: row.value,
        begin: row.time,
        end: nextTime ?? timeRange?.end ?? row.time,
      }
    } else if (currentSegment.value === row.value) {
      // Extend current segment
      currentSegment.end = nextTime ?? timeRange?.end ?? row.time
    } else {
      // Close current segment at this row's time, start new one
      currentSegment.end = row.time
      segments.push(currentSegment)
      currentSegment = {
        value: row.value,
        begin: row.time,
        end: nextTime ?? timeRange?.end ?? row.time,
      }
    }
  }

  if (currentSegment) {
    segments.push(currentSegment)
  }

  return segments
}
