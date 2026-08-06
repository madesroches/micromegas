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
        const flatProps = flattenProperties(props)
        rawData.set(row.time, flatProps)
        Object.keys(flatProps).forEach(k => keysSet.add(k))
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
 * Expands object-valued properties into dot-separated leaf entries with
 * string values (e.g. `Dimensions: {DBInstanceIdentifier: "foo"}` becomes
 * `"Dimensions.DBInstanceIdentifier": "foo"`). Array values are JSON-stringified
 * at the top level too, matching the nested behavior in `flattenObjectInto`, so
 * consumers never end up rendering `[object Object]` via `String(value)`.
 */
export function flattenProperties(props: Record<string, unknown>): Record<string, unknown> {
  const result: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(props)) {
    if (isPlainObject(value)) {
      flattenObjectInto(value, key, result)
    } else {
      result[key] = Array.isArray(value) ? JSON.stringify(value) : value
    }
  }
  return result
}

function flattenObjectInto(obj: Record<string, unknown>, prefix: string, result: Record<string, unknown>): void {
  for (const [key, value] of Object.entries(obj)) {
    const fullKey = `${prefix}.${key}`
    if (isPlainObject(value)) {
      flattenObjectInto(value, fullKey, result)
    } else if (value === null) {
      result[fullKey] = null
    } else {
      result[fullKey] = Array.isArray(value) ? JSON.stringify(value) : String(value)
    }
  }
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
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
