/**
 * Utilities for working with Apache Arrow data
 */

import { DataType, TimeUnit, Timestamp, Table } from 'apache-arrow'

export type XAxisMode = 'time' | 'numeric' | 'categorical'

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

/**
 * Check if an Arrow DataType is a time-related type
 */
export function isTimeType(dataType: DataType): boolean {
  return (
    DataType.isTimestamp(dataType) ||
    DataType.isDate(dataType) ||
    DataType.isTime(dataType)
  )
}

/**
 * Check if an Arrow DataType is a numeric type
 */
export function isNumericType(dataType: DataType): boolean {
  return (
    DataType.isInt(dataType) ||
    DataType.isFloat(dataType) ||
    DataType.isDecimal(dataType)
  )
}

/**
 * Check if an Arrow DataType is a string type
 */
export function isStringType(dataType: DataType): boolean {
  return DataType.isUtf8(dataType) || DataType.isLargeUtf8(dataType)
}

/**
 * Detect X-axis mode from Arrow column type
 */
export function detectXAxisMode(dataType: DataType): XAxisMode {
  if (isTimeType(dataType)) return 'time'
  if (isNumericType(dataType)) return 'numeric'
  if (isStringType(dataType)) return 'categorical'
  // Default to categorical for unsupported types
  return 'categorical'
}

/**
 * Validate that a table has exactly 2 columns with valid types for charting
 */
export function validateChartColumns(table: Table):
  | { valid: true; xType: DataType; yType: DataType }
  | { valid: false; error: string } {
  const fields = table.schema.fields

  if (fields.length !== 2) {
    return {
      valid: false,
      error: `Query must return exactly 2 columns (X and Y axis), got ${fields.length}`,
    }
  }

  const xType = fields[0].type
  const yType = fields[1].type

  // X column must be timestamp, numeric, or string
  if (!isTimeType(xType) && !isNumericType(xType) && !isStringType(xType)) {
    return {
      valid: false,
      error: 'First column must be timestamp, numeric, or string type for X-axis',
    }
  }

  // Y column must be numeric
  if (!isNumericType(yType)) {
    return {
      valid: false,
      error: 'Second column must be numeric type for Y-axis',
    }
  }

  return { valid: true, xType, yType }
}

/**
 * Extract chart data from Arrow table (first 2 columns)
 */
export function extractChartData(table: Table):
  | {
      ok: true
      data: { x: number; y: number }[]
      xAxisMode: XAxisMode
      xLabels?: string[] // for categorical - unique labels in SQL order
      xColumnName: string
      yColumnName: string
    }
  | { ok: false; error: string } {
  const validation = validateChartColumns(table)
  if (!validation.valid) {
    return { ok: false, error: validation.error }
  }

  const { xType, yType: _yType } = validation
  const fields = table.schema.fields
  const xColumnName = fields[0].name
  const yColumnName = fields[1].name
  const xAxisMode = detectXAxisMode(xType)

  const data: { x: number; y: number }[] = []

  if (xAxisMode === 'categorical') {
    // For categorical, build label array and map strings to indices
    const labelMap = new Map<string, number>()
    const xLabels: string[] = []

    for (let i = 0; i < table.numRows; i++) {
      const row = table.get(i)
      if (!row) continue

      const xVal = row[xColumnName]
      const yVal = row[yColumnName]

      // Skip rows with null values
      if (xVal == null || yVal == null) continue

      const str = String(xVal)
      const yNum = Number(yVal)

      if (isNaN(yNum)) continue

      if (!labelMap.has(str)) {
        labelMap.set(str, xLabels.length)
        xLabels.push(str)
      }

      data.push({ x: labelMap.get(str)!, y: yNum })
    }

    if (data.length === 0) {
      return { ok: false, error: 'No valid data points (all values are null)' }
    }

    // Categorical: preserve SQL order (don't sort)
    return { ok: true, data, xAxisMode, xLabels, xColumnName, yColumnName }
  } else {
    // Time or numeric mode
    for (let i = 0; i < table.numRows; i++) {
      const row = table.get(i)
      if (!row) continue

      const xVal = row[xColumnName]
      const yVal = row[yColumnName]

      // Skip rows with null values
      if (xVal == null || yVal == null) continue

      let xNum: number
      if (xAxisMode === 'time') {
        // Convert timestamp to milliseconds
        xNum = timestampToMs(xVal, xType)
      } else {
        xNum = Number(xVal)
      }

      const yNum = Number(yVal)

      if (isNaN(xNum) || isNaN(yNum)) continue

      data.push({ x: xNum, y: yNum })
    }

    if (data.length === 0) {
      return { ok: false, error: 'No valid data points (all values are null)' }
    }

    // Time/numeric: sort by X ascending (uPlot requirement)
    data.sort((a, b) => a.x - b.x)

    return { ok: true, data, xAxisMode, xColumnName, yColumnName }
  }
}
