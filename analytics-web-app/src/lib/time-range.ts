// Time range utilities for URL-based state management

// Re-export default time range from centralized defaults
export { DEFAULT_TIME_RANGE } from './screen-defaults'

export interface TimeRange {
  from: string
  to: string
}

export interface ParsedTimeRange {
  from: Date
  to: Date
  label: string
  isRelative: boolean
}

// Relative time range presets
export const TIME_RANGE_PRESETS = [
  { label: 'Last 5 minutes', value: 'now-5m', duration: 5 * 60 * 1000 },
  { label: 'Last 15 minutes', value: 'now-15m', duration: 15 * 60 * 1000 },
  { label: 'Last 30 minutes', value: 'now-30m', duration: 30 * 60 * 1000 },
  { label: 'Last 1 hour', value: 'now-1h', duration: 60 * 60 * 1000 },
  { label: 'Last 3 hours', value: 'now-3h', duration: 3 * 60 * 60 * 1000 },
  { label: 'Last 6 hours', value: 'now-6h', duration: 6 * 60 * 60 * 1000 },
  { label: 'Last 12 hours', value: 'now-12h', duration: 12 * 60 * 60 * 1000 },
  { label: 'Last 24 hours', value: 'now-24h', duration: 24 * 60 * 60 * 1000 },
  { label: 'Last 2 days', value: 'now-2d', duration: 2 * 24 * 60 * 60 * 1000 },
  { label: 'Last 7 days', value: 'now-7d', duration: 7 * 24 * 60 * 60 * 1000 },
  { label: 'Last 30 days', value: 'now-30d', duration: 30 * 24 * 60 * 60 * 1000 },
  { label: 'Last 90 days', value: 'now-90d', duration: 90 * 24 * 60 * 60 * 1000 },
] as const


// Time unit multipliers in milliseconds
const TIME_UNIT_MS: Record<string, number> = {
  s: 1000,
  m: 60 * 1000,
  h: 60 * 60 * 1000,
  d: 24 * 60 * 60 * 1000,
  w: 7 * 24 * 60 * 60 * 1000,
}

// Regex for relative time expressions: now, now-1h, now-30m, etc.
const RELATIVE_TIME_REGEX = /^now(-(\d+)([smhdw]))?$/

// Parse a relative time string like "now-1h" to a Date
export function parseRelativeTime(value: string, referenceTime: Date = new Date()): Date {
  if (value === 'now') {
    return referenceTime
  }

  const match = value.match(RELATIVE_TIME_REGEX)
  if (!match) {
    // Try parsing as ISO date
    const date = new Date(value)
    if (!isNaN(date.getTime())) {
      return date
    }
    throw new Error(`Invalid time value: ${value}`)
  }

  const amount = parseInt(match[2], 10)
  const unit = match[3]
  const ms = referenceTime.getTime()
  const unitMs = TIME_UNIT_MS[unit]

  if (!unitMs) {
    throw new Error(`Unknown time unit: ${unit}`)
  }

  return new Date(ms - amount * unitMs)
}

// Check if a time value is relative (e.g., "now-1h")
export function isRelativeTime(value: string): boolean {
  return RELATIVE_TIME_REGEX.test(value)
}

// Validate a time expression (relative or absolute ISO date)
export function isValidTimeExpression(value: string): boolean {
  if (isRelativeTime(value)) {
    return true
  }
  const date = new Date(value)
  return !isNaN(date.getTime())
}

// Format a relative time expression to human-readable string
// "now-1h" -> "Last 1 hour"
// "now-90m" -> "Last 90 minutes"
export function formatRelativeTime(value: string): string {
  if (value === 'now') {
    return 'Now'
  }

  const match = value.match(RELATIVE_TIME_REGEX)
  if (!match) {
    return value
  }

  const amount = parseInt(match[2], 10)
  const unit = match[3]

  const unitNames: Record<string, { singular: string; plural: string }> = {
    s: { singular: 'second', plural: 'seconds' },
    m: { singular: 'minute', plural: 'minutes' },
    h: { singular: 'hour', plural: 'hours' },
    d: { singular: 'day', plural: 'days' },
    w: { singular: 'week', plural: 'weeks' },
  }

  const unitName = unitNames[unit]
  if (!unitName) {
    return value
  }

  const name = amount === 1 ? unitName.singular : unitName.plural
  return `Last ${amount} ${name}`
}

// Parse time range from URL params
export function parseTimeRange(from: string, to: string): ParsedTimeRange {
  const now = new Date()
  const fromDate = parseRelativeTime(from, now)
  const toDate = parseRelativeTime(to, now)
  const isRelative = isRelativeTime(from) || isRelativeTime(to)

  // Find matching preset label
  let label = ''
  if (isRelative && to === 'now') {
    const preset = TIME_RANGE_PRESETS.find((p) => p.value === from)
    if (preset) {
      label = preset.label
    } else {
      label = `${from} to ${to}`
    }
  } else {
    // Format absolute dates
    const formatDate = (d: Date) =>
      d.toLocaleDateString('en-US', {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      })
    label = `${formatDate(fromDate)} - ${formatDate(toDate)}`
  }

  return {
    from: fromDate,
    to: toDate,
    label,
    isRelative,
  }
}

// Format Date to ISO string for API calls (UTC)
export function formatTimeForApi(date: Date): string {
  return date.toISOString()
}

// Format Date for display in local timezone (YYYY-MM-DD HH:mm:ss)
export function formatDateTimeLocal(date: Date): string {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  const seconds = String(date.getSeconds()).padStart(2, '0')
  return `${year}-${month}-${day} ${hours}:${minutes}:${seconds}`
}

// Get time range for API requests
export function getTimeRangeForApi(from: string, to: string): { begin: string; end: string } {
  const parsed = parseTimeRange(from, to)
  return {
    begin: formatTimeForApi(parsed.from),
    end: formatTimeForApi(parsed.to),
  }
}

const MIN_ZOOM_DURATION_MS = 1 // 1 millisecond
const MAX_ZOOM_DURATION_MS = 365 * 24 * 60 * 60 * 1000 // 365 days

// Zoom the time range in or out, centered on the current view.
// Zoom out doubles the duration, zoom in halves it.
// Always returns absolute ISO strings.
export function zoomTimeRange(
  from: string,
  to: string,
  direction: 'in' | 'out'
): { from: string; to: string } {
  const parsed = parseTimeRange(from, to)
  const fromMs = parsed.from.getTime()
  const toMs = parsed.to.getTime()
  let duration = toMs - fromMs

  // Handle zero-duration edge case
  if (duration === 0) {
    duration = 30 * 1000
  }

  const center = fromMs + duration / 2
  const newDuration =
    direction === 'out'
      ? Math.min(duration * 2, MAX_ZOOM_DURATION_MS)
      : Math.max(duration / 2, MIN_ZOOM_DURATION_MS)

  let newFrom = center - newDuration / 2
  let newTo = center + newDuration / 2

  // Clamp: don't go into the future
  const now = Date.now()
  if (newTo > now) {
    newTo = now
    newFrom = newTo - newDuration
  }

  return {
    from: new Date(newFrom).toISOString(),
    to: new Date(newTo).toISOString(),
  }
}

// Format duration between two timestamps
export function formatDuration(
  startTime: string | Date | unknown,
  endTime: string | Date | unknown
): string {
  const start = toDate(startTime)
  const end = toDate(endTime)
  if (!start || !end) return 'N/A'

  const diffMs = end.getTime() - start.getTime()

  if (isNaN(diffMs) || diffMs < 0) return 'Invalid'

  return formatDurationMs(diffMs)
}

// Format a duration in milliseconds as a human-readable string
export function formatDurationMs(ms: number): string {
  if (isNaN(ms) || ms < 0) return 'Invalid'

  const totalSeconds = Math.floor(ms / 1000)
  const seconds = totalSeconds % 60
  const minutes = Math.floor(totalSeconds / 60) % 60
  const hours = Math.floor(totalSeconds / 3600) % 24
  const days = Math.floor(totalSeconds / 86400)

  if (days > 0) {
    return `${days}d ${hours}h ${minutes}m`
  } else if (hours > 0) {
    return `${hours}h ${minutes}m ${seconds}s`
  } else if (minutes > 0) {
    return `${minutes}m ${seconds}s`
  } else {
    return `${seconds}s`
  }
}

// Convert unknown value to Date, handling Arrow BigInt timestamps
function toDate(value: unknown): Date | null {
  if (!value) return null
  if (value instanceof Date) return value
  if (typeof value === 'number') return new Date(value)
  if (typeof value === 'bigint') {
    // Arrow timestamps are BigInt nanoseconds
    return new Date(Number(value / 1000000n))
  }
  const date = new Date(String(value))
  return isNaN(date.getTime()) ? null : date
}

// Format timestamp for display in tables (local timezone)
export function formatTimestamp(value: unknown): string {
  const date = toDate(value)
  if (!date) return ''
  return formatDateTimeLocal(date)
}
