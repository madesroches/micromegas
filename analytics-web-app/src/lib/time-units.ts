/**
 * Adaptive time unit formatting
 *
 * Converts time values to the most appropriate unit based on the value range.
 * For example, 0.02 seconds -> 20 ms, 1500 ns -> 1.5 µs
 */

export type TimeUnit =
  | 'nanoseconds'
  | 'microseconds'
  | 'milliseconds'
  | 'seconds'
  | 'minutes'
  | 'hours'
  | 'days'

interface TimeUnitInfo {
  unit: TimeUnit
  abbrev: string
  factor: number
}

// Time units in ascending order of magnitude (all relative to nanoseconds)
const TIME_UNITS: TimeUnitInfo[] = [
  { unit: 'nanoseconds', abbrev: 'ns', factor: 1 },
  { unit: 'microseconds', abbrev: 'µs', factor: 1e3 },
  { unit: 'milliseconds', abbrev: 'ms', factor: 1e6 },
  { unit: 'seconds', abbrev: 's', factor: 1e9 },
  { unit: 'minutes', abbrev: 'min', factor: 60e9 },
  { unit: 'hours', abbrev: 'h', factor: 3600e9 },
  { unit: 'days', abbrev: 'd', factor: 86400e9 },
]

/**
 * Check if a unit is a time-based unit
 */
export function isTimeUnit(unit: string): unit is TimeUnit {
  return TIME_UNITS.some((t) => t.unit === unit)
}

/**
 * Get the factor to convert from a time unit to nanoseconds
 */
function getUnitFactor(unit: TimeUnit): number {
  return TIME_UNITS.find((t) => t.unit === unit)?.factor ?? 1e9
}

/**
 * Convert a value from one time unit to nanoseconds
 */
function toNanoseconds(value: number, fromUnit: TimeUnit): number {
  return value * getUnitFactor(fromUnit)
}

export interface AdaptiveTimeUnit {
  unit: TimeUnit
  abbrev: string
  conversionFactor: number // multiply original values by this to get values in new unit
}

/**
 * Determine the best time unit to display values based on a reference value (e.g., p99 or max).
 *
 * The logic selects a unit where the reference value falls in a readable range (1-999).
 *
 * @param referenceValue - A representative value (e.g., p99, max) in the original unit
 * @param originalUnit - The original unit of the values
 * @returns The best unit to use for display
 */
export function getAdaptiveTimeUnit(
  referenceValue: number,
  originalUnit: TimeUnit
): AdaptiveTimeUnit {
  // Convert reference value to nanoseconds
  const refNs = toNanoseconds(referenceValue, originalUnit)

  // Find the best unit where the value is >= 1 (prefer larger units when readable)
  // Work backwards from largest unit to find the smallest unit where value >= 1
  let bestUnit = TIME_UNITS[0]

  for (let i = TIME_UNITS.length - 1; i >= 0; i--) {
    const unit = TIME_UNITS[i]
    const convertedValue = refNs / unit.factor
    if (convertedValue >= 1) {
      bestUnit = unit
      break
    }
  }

  // Calculate the conversion factor from original unit to best unit
  const originalFactor = getUnitFactor(originalUnit)
  const bestFactor = bestUnit.factor
  const conversionFactor = originalFactor / bestFactor

  return {
    unit: bestUnit.unit,
    abbrev: bestUnit.abbrev,
    conversionFactor,
  }
}

/**
 * Convert a value from the original unit to the adaptive unit
 */
export function convertToAdaptiveUnit(
  value: number,
  originalUnit: TimeUnit,
  adaptiveUnit: AdaptiveTimeUnit
): number {
  return value * adaptiveUnit.conversionFactor
}

/**
 * Format a time value with adaptive units.
 *
 * @param value - The value in the original unit
 * @param adaptiveUnit - The adaptive unit info (from getAdaptiveTimeUnit)
 * @param abbreviated - Whether to use abbreviated unit (ms vs milliseconds)
 * @returns Formatted string like "20 ms" or "20 milliseconds"
 */
export function formatAdaptiveTime(
  value: number,
  adaptiveUnit: AdaptiveTimeUnit,
  abbreviated = false
): string {
  const convertedValue = value * adaptiveUnit.conversionFactor
  const unitStr = abbreviated ? adaptiveUnit.abbrev : adaptiveUnit.unit

  // Format with appropriate precision based on magnitude
  const absValue = Math.abs(convertedValue)
  if (absValue >= 1000) {
    return Math.round(convertedValue).toLocaleString() + ' ' + unitStr
  } else if (absValue >= 100) {
    return convertedValue.toFixed(0) + ' ' + unitStr
  } else if (absValue >= 10) {
    return convertedValue.toFixed(1) + ' ' + unitStr
  } else if (absValue >= 1) {
    return convertedValue.toFixed(2) + ' ' + unitStr
  } else if (absValue >= 0.1) {
    return convertedValue.toFixed(2) + ' ' + unitStr
  } else if (absValue === 0) {
    return '0 ' + unitStr
  } else {
    // Very small values - use exponential notation
    return convertedValue.toPrecision(2) + ' ' + unitStr
  }
}

/**
 * Format a single time value, automatically choosing the best unit for that specific value.
 * Use this for stats (min, max, avg) where each value should pick its own appropriate unit.
 *
 * @param value - The value in the original unit
 * @param originalUnit - The original unit of the value
 * @param abbreviated - Whether to use abbreviated unit (ms vs milliseconds)
 * @returns Formatted string like "20 ms" or "1.5 seconds"
 */
export function formatTimeValue(
  value: number,
  originalUnit: TimeUnit,
  abbreviated = false
): string {
  const adaptive = getAdaptiveTimeUnit(value, originalUnit)
  return formatAdaptiveTime(value, adaptive, abbreviated)
}
