import { PropertySegment } from '@/types'

/**
 * Parse interval string (e.g., "50 milliseconds", "1 second") to milliseconds.
 * Fixed to handle millisecond intervals that were previously unsupported.
 */
export function parseIntervalToMs(interval: string): number {
  const match = interval.match(/^(\d+)\s*(millisecond|second|minute|hour|day)s?$/i)
  if (!match) return 60000 // default to 1 minute

  const value = parseInt(match[1], 10)
  const unit = match[2].toLowerCase()

  switch (unit) {
    case 'millisecond':
      return value
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

/**
 * Aggregate time-value rows into contiguous segments where adjacent rows
 * with the same value are merged.
 */
export function aggregateIntoSegments(
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
