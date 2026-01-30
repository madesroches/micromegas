/**
 * Unit normalization and formatting utilities
 *
 * Converts various unit aliases to canonical form for consistent handling.
 */

export const UNIT_ALIASES: Record<string, string> = {
  // Time (include canonical names for case-insensitive matching)
  'ns': 'nanoseconds',
  'nanoseconds': 'nanoseconds',
  'Nanoseconds': 'nanoseconds',
  'µs': 'microseconds',
  'us': 'microseconds',
  'microseconds': 'microseconds',
  'Microseconds': 'microseconds',
  'ms': 'milliseconds',
  'milliseconds': 'milliseconds',
  'Milliseconds': 'milliseconds',
  's': 'seconds',
  'seconds': 'seconds',
  'Seconds': 'seconds',
  'min': 'minutes',
  'minutes': 'minutes',
  'Minutes': 'minutes',
  'h': 'hours',
  'hours': 'hours',
  'Hours': 'hours',
  'd': 'days',
  'days': 'days',
  'Days': 'days',
  // Size
  'bytes': 'bytes',
  'Bytes': 'bytes',
  'B': 'bytes',
  'kilobytes': 'kilobytes',
  'Kilobytes': 'kilobytes',
  'KB': 'kilobytes',
  'kb': 'kilobytes',
  'megabytes': 'megabytes',
  'Megabytes': 'megabytes',
  'MB': 'megabytes',
  'gigabytes': 'gigabytes',
  'Gigabytes': 'gigabytes',
  'GB': 'gigabytes',
  // Rate
  'BytesPerSecond': 'bytes/s',
  'BytesPerSeconds': 'bytes/s',
  'B/s': 'bytes/s',
  'bytes/s': 'bytes/s',
  // Other
  'requests': 'count',
  'count': 'count',
  '%': 'percent',
  'percent': 'percent',
  'deg': 'degrees',
  'degrees': 'degrees',
  'boolean': 'boolean',
  'none': 'none',
  '': 'none',
}

/**
 * Normalize a unit string to its canonical form.
 * Returns the original unit if no alias is found.
 */
export function normalizeUnit(unit: string): string {
  return UNIT_ALIASES[unit] ?? unit
}

/**
 * Set of canonical time unit names
 */
export const TIME_UNIT_NAMES = new Set([
  'nanoseconds',
  'microseconds',
  'milliseconds',
  'seconds',
  'minutes',
  'hours',
  'days',
])
