// Time range utilities for URL-based state management

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
  { label: 'Last 1 hour', value: 'now-1h', duration: 60 * 60 * 1000 },
  { label: 'Last 6 hours', value: 'now-6h', duration: 6 * 60 * 60 * 1000 },
  { label: 'Last 12 hours', value: 'now-12h', duration: 12 * 60 * 60 * 1000 },
  { label: 'Last 24 hours', value: 'now-24h', duration: 24 * 60 * 60 * 1000 },
  { label: 'Last 7 days', value: 'now-7d', duration: 7 * 24 * 60 * 60 * 1000 },
  { label: 'Last 30 days', value: 'now-30d', duration: 30 * 24 * 60 * 60 * 1000 },
] as const

export const DEFAULT_TIME_RANGE: TimeRange = {
  from: 'now-24h',
  to: 'now',
}

// Parse a relative time string like "now-1h" to a Date
export function parseRelativeTime(value: string, referenceTime: Date = new Date()): Date {
  if (value === 'now') {
    return referenceTime
  }

  const match = value.match(/^now-(\d+)([mhd])$/)
  if (!match) {
    // Try parsing as ISO date
    const date = new Date(value)
    if (!isNaN(date.getTime())) {
      return date
    }
    throw new Error(`Invalid time value: ${value}`)
  }

  const amount = parseInt(match[1], 10)
  const unit = match[2]
  const ms = referenceTime.getTime()

  switch (unit) {
    case 'm':
      return new Date(ms - amount * 60 * 1000)
    case 'h':
      return new Date(ms - amount * 60 * 60 * 1000)
    case 'd':
      return new Date(ms - amount * 24 * 60 * 60 * 1000)
    default:
      throw new Error(`Unknown time unit: ${unit}`)
  }
}

// Check if a time value is relative (e.g., "now-1h")
export function isRelativeTime(value: string): boolean {
  return value === 'now' || /^now-\d+[mhd]$/.test(value)
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

// Format Date to ISO string for API calls
export function formatTimeForApi(date: Date): string {
  return date.toISOString()
}

// Get time range for API requests
export function getTimeRangeForApi(from: string, to: string): { begin: string; end: string } {
  const parsed = parseTimeRange(from, to)
  return {
    begin: formatTimeForApi(parsed.from),
    end: formatTimeForApi(parsed.to),
  }
}

// Format duration between two timestamps
export function formatDuration(
  startTime: string | Date | unknown,
  endTime: string | Date | unknown
): string {
  if (!startTime || !endTime) return 'N/A'

  const start = startTime instanceof Date ? startTime : new Date(String(startTime))
  const end = endTime instanceof Date ? endTime : new Date(String(endTime))
  const diffMs = end.getTime() - start.getTime()

  if (isNaN(diffMs) || diffMs < 0) return 'Invalid'

  const totalSeconds = Math.floor(diffMs / 1000)
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

// Format timestamp for display in tables (ISO format with space separator)
export function formatTimestamp(value: unknown): string {
  if (!value) return ''
  const date = new Date(String(value))
  if (isNaN(date.getTime())) return ''
  return date.toISOString().replace('T', ' ').slice(0, 23) + 'Z'
}
