/**
 * Utilities for working with Apache Arrow data
 */

import { DataType, TimeUnit, Timestamp } from 'apache-arrow'

/**
 * Converts an Arrow timestamp value to milliseconds.
 * Uses the field's schema to determine the correct conversion factor.
 */
export function timestampToMs(value: unknown, dataType?: DataType): number {
  if (!value) return 0
  if (value instanceof Date) return value.getTime()

  if (typeof value === 'number') {
    // Use the schema's time unit if available
    if (dataType && DataType.isTimestamp(dataType)) {
      const timestampType = dataType as Timestamp
      switch (timestampType.unit) {
        case TimeUnit.SECOND:
          return value * 1000
        case TimeUnit.MILLISECOND:
          return value
        case TimeUnit.MICROSECOND:
          return value / 1000
        case TimeUnit.NANOSECOND:
          return value / 1000000
      }
    }
    // No dataType - assume milliseconds
    return value
  }

  if (typeof value === 'bigint') {
    // Determine divisor based on the Arrow type's time unit
    if (dataType && DataType.isTimestamp(dataType)) {
      const timestampType = dataType as Timestamp
      switch (timestampType.unit) {
        case TimeUnit.SECOND:
          return Number(value) * 1000
        case TimeUnit.MILLISECOND:
          return Number(value)
        case TimeUnit.MICROSECOND:
          return Number(value / 1000n)
        case TimeUnit.NANOSECOND:
          return Number(value / 1000000n)
      }
    }
    // Default: assume nanoseconds (most common in micromegas)
    return Number(value / 1000000n)
  }

  // Try parsing as string
  const date = new Date(String(value))
  return isNaN(date.getTime()) ? 0 : date.getTime()
}

/**
 * Converts an Arrow timestamp value to a Date object.
 * Uses the field's schema to determine the correct conversion factor.
 */
export function timestampToDate(value: unknown, dataType?: DataType): Date | null {
  if (!value) return null
  if (value instanceof Date) return value

  if (typeof value === 'number' || typeof value === 'bigint') {
    const ms = timestampToMs(value, dataType)
    return new Date(ms)
  }

  // Try parsing as string
  const date = new Date(String(value))
  return isNaN(date.getTime()) ? null : date
}
