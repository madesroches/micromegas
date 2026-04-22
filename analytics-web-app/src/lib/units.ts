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
  // Bits (networking, decimal scaling)
  'bit': 'bits',
  'bits': 'bits',
  'Bits': 'bits',
  'kbit': 'kilobits',
  'kbits': 'kilobits',
  'kilobits': 'kilobits',
  'Kilobits': 'kilobits',
  'Mbit': 'megabits',
  'Mbits': 'megabits',
  'megabits': 'megabits',
  'Megabits': 'megabits',
  'Gbit': 'gigabits',
  'Gbits': 'gigabits',
  'gigabits': 'gigabits',
  'Gigabits': 'gigabits',
  'Tbit': 'terabits',
  'Tbits': 'terabits',
  'terabits': 'terabits',
  'Terabits': 'terabits',
  // Rate
  'BytesPerSecond': 'bytes/s',
  'BytesPerSeconds': 'bytes/s',
  'B/s': 'bytes/s',
  'bytes/s': 'bytes/s',
  'bit/s': 'bits/s',
  'bits/s': 'bits/s',
  'bps': 'bits/s',
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

/**
 * Set of canonical bit unit names (networking, decimal scaling)
 */
export const BIT_UNIT_NAMES = new Set([
  'bits',
  'kilobits',
  'megabits',
  'gigabits',
  'terabits',
])

/**
 * Check if a unit (or its alias) is a bit-based unit
 */
export function isBitUnit(unit: string): boolean {
  return BIT_UNIT_NAMES.has(normalizeUnit(unit))
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

export type BitUnit = 'bits' | 'kilobits' | 'megabits' | 'gigabits' | 'terabits'

interface BitUnitInfo {
  unit: BitUnit
  abbrev: string
  factor: number // multiplier to convert to bits
}

// Decimal bit units (power of 10, networking convention)
const KBIT = 1000
const MBIT = KBIT * 1000
const GBIT = MBIT * 1000
const TBIT = GBIT * 1000

const BIT_UNITS: BitUnitInfo[] = [
  { unit: 'bits', abbrev: 'bit', factor: 1 },
  { unit: 'kilobits', abbrev: 'kbit', factor: KBIT },
  { unit: 'megabits', abbrev: 'Mbit', factor: MBIT },
  { unit: 'gigabits', abbrev: 'Gbit', factor: GBIT },
  { unit: 'terabits', abbrev: 'Tbit', factor: TBIT },
]

export interface AdaptiveBitUnit {
  unit: BitUnit
  abbrev: string
  conversionFactor: number
}

function getBitUnitFactor(unit: BitUnit): number {
  const info = BIT_UNITS.find((u) => u.unit === unit)
  return info?.factor ?? 1
}

function toBits(value: number, unit: BitUnit): number {
  return value * getBitUnitFactor(unit)
}

/**
 * Determine the best bit unit to display a reference value.
 * Uses decimal scaling (1 kbit = 1000 bits) per networking convention.
 */
export function getAdaptiveBitUnit(
  referenceValue: number,
  originalUnit: BitUnit | string
): AdaptiveBitUnit {
  const normalizedUnit = normalizeUnit(originalUnit) as BitUnit
  const refBits = toBits(referenceValue, normalizedUnit)

  let bestUnit = BIT_UNITS[0]
  for (let i = BIT_UNITS.length - 1; i >= 0; i--) {
    const u = BIT_UNITS[i]
    const valueInUnit = refBits / u.factor
    if (valueInUnit >= 1) {
      bestUnit = u
      break
    }
  }

  const originalFactor = getBitUnitFactor(normalizedUnit)
  const conversionFactor = originalFactor / bestUnit.factor

  return {
    unit: bestUnit.unit,
    abbrev: bestUnit.abbrev,
    conversionFactor,
  }
}
