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
  'terabytes': 'terabytes',
  'Terabytes': 'terabytes',
  'TB': 'terabytes',
  // Rate
  'BytesPerSecond': 'bytes/s',
  'BytesPerSeconds': 'bytes/s',
  'B/s': 'bytes/s',
  'bytes/s': 'bytes/s',
  // Other
  '%': 'percent',
  'percent': 'percent',
  'deg': 'degrees',
  'degrees': 'degrees',
  'boolean': 'boolean',
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

/**
 * Set of canonical size unit names
 */
export const SIZE_UNIT_NAMES = new Set([
  'bytes',
  'kilobytes',
  'megabytes',
  'gigabytes',
  'terabytes',
])

/**
 * Check if a unit (or its alias) is a size-based unit
 */
export function isSizeUnit(unit: string): boolean {
  return SIZE_UNIT_NAMES.has(normalizeUnit(unit))
}

export type SizeUnit = 'bytes' | 'kilobytes' | 'megabytes' | 'gigabytes' | 'terabytes'

interface SizeUnitInfo {
  unit: SizeUnit
  abbrev: string
  factor: number // multiplier to convert to bytes
}

// Binary size units (power of 2)
const KB = 1024
const MB = KB * 1024
const GB = MB * 1024
const TB = GB * 1024

const SIZE_UNITS: SizeUnitInfo[] = [
  { unit: 'bytes', abbrev: 'B', factor: 1 },
  { unit: 'kilobytes', abbrev: 'KB', factor: KB },
  { unit: 'megabytes', abbrev: 'MB', factor: MB },
  { unit: 'gigabytes', abbrev: 'GB', factor: GB },
  { unit: 'terabytes', abbrev: 'TB', factor: TB },
]

export interface AdaptiveSizeUnit {
  unit: SizeUnit
  abbrev: string
  conversionFactor: number // multiply original value by this to get display value
}

/**
 * Get the unit factor (bytes per unit)
 */
function getSizeUnitFactor(unit: SizeUnit): number {
  const info = SIZE_UNITS.find((u) => u.unit === unit)
  return info?.factor ?? 1
}

/**
 * Convert a value to bytes from any size unit
 */
function toBytes(value: number, unit: SizeUnit): number {
  return value * getSizeUnitFactor(unit)
}

/**
 * Determine the best size unit to display a reference value.
 * Picks a unit where the value falls in a readable range (1-999).
 *
 * @param referenceValue - A representative value (e.g., p99, max) in the original unit
 * @param originalUnit - The original unit of the values (can be an alias)
 * @returns The best unit to use for display
 */
export function getAdaptiveSizeUnit(
  referenceValue: number,
  originalUnit: SizeUnit | string
): AdaptiveSizeUnit {
  const normalizedUnit = normalizeUnit(originalUnit) as SizeUnit
  const refBytes = toBytes(referenceValue, normalizedUnit)

  // Find the best unit where the value is >= 1 (prefer larger units)
  let bestUnit = SIZE_UNITS[0]
  for (let i = SIZE_UNITS.length - 1; i >= 0; i--) {
    const u = SIZE_UNITS[i]
    const valueInUnit = refBytes / u.factor
    if (valueInUnit >= 1) {
      bestUnit = u
      break
    }
  }

  // Calculate the conversion factor from original unit to best unit
  const originalFactor = getSizeUnitFactor(normalizedUnit)
  const bestFactor = bestUnit.factor
  const conversionFactor = originalFactor / bestFactor

  return {
    unit: bestUnit.unit,
    abbrev: bestUnit.abbrev,
    conversionFactor,
  }
}
