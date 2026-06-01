/**
 * Utilities for working with Apache Arrow data
 */

import { DataType, TimeUnit, Timestamp, Duration, Table } from 'apache-arrow'
import { cellColorToCss } from '@/lib/color-utils'

export type XAxisMode = 'time' | 'numeric' | 'categorical'

/**
 * Converts an Arrow timestamp value to milliseconds.
 * Uses the field's schema to determine the correct conversion factor.
 */
export function timestampToMs(value: unknown, dataType?: DataType): number {
  if (value == null) return NaN
  if (value instanceof Date) return value.getTime()

  // Arrow JS automatically converts all timestamp types to milliseconds when
  // deserializing to JavaScript Numbers (since JS can't precisely represent
  // nanosecond-precision timestamps as Numbers). So for numeric values,
  // we always treat them as milliseconds regardless of what the schema says.
  if (typeof value === 'number') {
    return value
  }

  // Bigints may still come through with original precision, so use schema
  if (typeof value === 'bigint') {
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
  return date.getTime() // Returns NaN for invalid dates
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
 * Check if an Arrow DataType is a duration type
 */
export function isDurationType(dataType: DataType): boolean {
  return DataType.isDuration(dataType)
}

/**
 * Converts an Arrow duration value to milliseconds.
 * Uses the field's schema to determine the correct conversion factor.
 */
export function durationToMs(value: unknown, dataType?: DataType): number {
  if (!value) return 0

  // Convert to Number early to preserve fractional milliseconds
  const numValue = typeof value === 'bigint' ? Number(value) : Number(value)

  if (dataType && DataType.isDuration(dataType)) {
    const durationType = dataType as Duration
    switch (durationType.unit) {
      case TimeUnit.SECOND:
        return numValue * 1000
      case TimeUnit.MILLISECOND:
        return numValue
      case TimeUnit.MICROSECOND:
        return numValue / 1000
      case TimeUnit.NANOSECOND:
        return numValue / 1000000
    }
  }

  // Default: assume nanoseconds
  return numValue / 1000000
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
 * Check if an Arrow DataType is an integer type.
 * Tighter than isNumericType (excludes Float/Decimal) — for paths that need
 * bit-exact integer semantics, e.g. an integer column read as RGBA u32.
 * Like isNumericType / isStringType, this does NOT unwrap dictionaries —
 * callers wrap with unwrapDictionary themselves.
 */
export function isIntegerType(dataType: DataType): boolean {
  return DataType.isInt(dataType)
}

/**
 * Check if an Arrow DataType is a string type
 */
export function isStringType(dataType: DataType): boolean {
  return DataType.isUtf8(dataType) || DataType.isLargeUtf8(dataType)
}

/**
 * Get the underlying type, unwrapping dictionary encoding if present.
 * Dictionary-encoded columns store indices that reference a dictionary of values.
 * This function returns the value type (e.g., Utf8 from Dictionary<Int32, Utf8>).
 */
export function unwrapDictionary(dataType: DataType): DataType {
  if (DataType.isDictionary(dataType)) {
    // Dictionary type has a 'dictionary' property with the value type
    return (dataType as { dictionary: DataType }).dictionary
  }
  return dataType
}

/**
 * Check if an Arrow DataType is a binary type (handles dictionary-encoded binary).
 */
export function isBinaryType(dataType: DataType): boolean {
  const inner = unwrapDictionary(dataType)
  return (
    DataType.isBinary(inner) ||
    DataType.isLargeBinary(inner) ||
    DataType.isFixedSizeBinary(inner)
  )
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

// =============================================================================
// Chart Point Type
// =============================================================================

export interface ChartPoint {
  x: number
  y: number
  /** CSS color decoded from the SQL `color` column, if present. */
  color?: string
}

// =============================================================================
// Color Column Resolution
// =============================================================================

type ColorColumnKind = 'integer' | 'string' | 'binary'

interface ResolvedChartColumns {
  xColumnName: string
  yColumnName: string
  xType: DataType
  yType: DataType
  colorColumnName?: string
  colorColumnKind?: ColorColumnKind
}

/**
 * Resolve X, Y, and optional color columns from a schema field list.
 * The color column is the field named 'color' (case-insensitive).
 * X and Y are the first two non-color fields in order.
 * Callers must ensure there are at least 2 non-color fields.
 */
export function resolveChartColumns(
  fields: { name: string; type: DataType }[]
): ResolvedChartColumns {
  const colorIdx = fields.findIndex(f => f.name.toLowerCase() === 'color')
  const nonColorFields = colorIdx >= 0
    ? fields.filter((_, i) => i !== colorIdx)
    : fields

  const xField = nonColorFields[0]
  const yField = nonColorFields[1]

  const result: ResolvedChartColumns = {
    xColumnName: xField.name,
    yColumnName: yField.name,
    xType: xField.type,
    yType: yField.type,
  }

  if (colorIdx >= 0) {
    const colorField = fields[colorIdx]
    const innerType = unwrapDictionary(colorField.type)
    result.colorColumnName = colorField.name
    if (isIntegerType(innerType)) {
      result.colorColumnKind = 'integer'
    } else if (isStringType(innerType)) {
      result.colorColumnKind = 'string'
    } else if (isBinaryType(colorField.type)) {
      result.colorColumnKind = 'binary'
    }
  }

  return result
}

/**
 * Validate that a table has X and Y columns (plus optional 'color') with valid types.
 * Returns resolved column names and types for callers that need them.
 */
export function validateChartColumns(table: Table):
  | {
      valid: true
      xType: DataType
      yType: DataType
      xColumnName: string
      yColumnName: string
      colorColumnName?: string
      colorColumnKind?: ColorColumnKind
    }
  | { valid: false; error: string } {
  const fields = table.schema.fields
  const colorIdx = fields.findIndex(f => f.name.toLowerCase() === 'color')
  const nonColorCount = colorIdx >= 0 ? fields.length - 1 : fields.length

  if (nonColorCount < 2) {
    return {
      valid: false,
      error: `Query must return X and Y columns, got ${nonColorCount} non-color columns`,
    }
  }

  if (nonColorCount > 2) {
    return {
      valid: false,
      error: `Query must return X and Y columns (plus optional 'color'), got ${nonColorCount} non-color columns`,
    }
  }

  const resolved = resolveChartColumns(fields)
  const { xType, yType, xColumnName, yColumnName, colorColumnName, colorColumnKind } = resolved

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

  // Validate color column type if present
  if (colorIdx >= 0) {
    const colorField = fields[colorIdx]
    const innerType = unwrapDictionary(colorField.type)
    if (!isIntegerType(innerType) && !isStringType(innerType) && !isBinaryType(colorField.type)) {
      return {
        valid: false,
        error: `'color' column must be integer (packed RGBA u32), string ('#rrggbb'/'#rrggbbaa'), or binary, got ${colorField.type.toString()}`,
      }
    }
  }

  return { valid: true, xType, yType, xColumnName, yColumnName, colorColumnName, colorColumnKind }
}

// =============================================================================
// Multi-Series Chart Types
// =============================================================================

export interface ChartSeriesData {
  label: string
  unit: string
  /** User-chosen series color; default = rotating palette by index. */
  color?: string
  data: ChartPoint[]
}

export interface MultiSeriesChartData {
  xAxisMode: XAxisMode
  xLabels?: string[]
  xColumnName: string
  series: ChartSeriesData[]
}

/**
 * Extract multi-series chart data from multiple Arrow tables.
 * Each table must have X and Y columns (plus optional 'color').
 * All tables must agree on X-axis mode.
 */
export function extractMultiSeriesChartData(
  tables: { table: Table; unit?: string; label?: string }[]
): ({ ok: true } & MultiSeriesChartData) | { ok: false; error: string } {
  if (tables.length === 0) {
    return { ok: false, error: 'No query results' }
  }

  // Validate each table and detect modes
  const validations: {
    xType: DataType
    yType: DataType
    xColumnName: string
    yColumnName: string
    xAxisMode: XAxisMode
    colorColumnName?: string
    colorColumnKind?: ColorColumnKind
  }[] = []

  for (let i = 0; i < tables.length; i++) {
    const { table } = tables[i]
    if (table.numRows === 0) {
      // Allow empty tables — they produce an empty series
      const fields = table.schema.fields
      const colorIdx = fields.findIndex(f => f.name.toLowerCase() === 'color')
      const nonColorCount = colorIdx >= 0 ? fields.length - 1 : fields.length
      if (nonColorCount !== 2) {
        return { ok: false, error: `Query ${i + 1}: must return X and Y columns, got ${nonColorCount} non-color columns` }
      }
      const resolved = resolveChartColumns(fields)
      validations.push({
        xType: resolved.xType,
        yType: resolved.yType,
        xColumnName: resolved.xColumnName,
        yColumnName: resolved.yColumnName,
        xAxisMode: detectXAxisMode(resolved.xType),
        colorColumnName: resolved.colorColumnName,
        colorColumnKind: resolved.colorColumnKind,
      })
      continue
    }
    const v = validateChartColumns(table)
    if (!v.valid) {
      return { ok: false, error: `Query ${i + 1}: ${v.error}` }
    }
    validations.push({
      xType: v.xType,
      yType: v.yType,
      xColumnName: v.xColumnName,
      yColumnName: v.yColumnName,
      xAxisMode: detectXAxisMode(v.xType),
      colorColumnName: v.colorColumnName,
      colorColumnKind: v.colorColumnKind,
    })
  }

  // All tables must agree on X-axis mode
  const xAxisMode = validations[0].xAxisMode
  for (let i = 1; i < validations.length; i++) {
    if (validations[i].xAxisMode !== xAxisMode) {
      return {
        ok: false,
        error: `X-axis mode mismatch: query 1 is ${xAxisMode}, query ${i + 1} is ${validations[i].xAxisMode}`,
      }
    }
  }

  const xColumnName = validations[0].xColumnName

  // Extract each series
  const series: ChartSeriesData[] = []
  for (let i = 0; i < tables.length; i++) {
    const { table, unit, label } = tables[i]
    const v = validations[i]
    const seriesLabel = label || v.yColumnName
    const seriesUnit = unit || ''

    const data: ChartPoint[] = []

    if (xAxisMode === 'categorical') {
      // For categorical, we'll handle label mapping after collecting all data.
      // Color is decoded in the second-pass remap loop below.
      for (let r = 0; r < table.numRows; r++) {
        const row = table.get(r)
        if (!row) continue
        const xVal = row[v.xColumnName]
        const yVal = row[v.yColumnName]
        if (xVal == null || yVal == null) continue
        const yNum = Number(yVal)
        if (isNaN(yNum)) continue
        data.push({ x: 0, y: yNum }) // placeholder x; rebuilt below
      }
    } else {
      for (let r = 0; r < table.numRows; r++) {
        const row = table.get(r)
        if (!row) continue
        const xVal = row[v.xColumnName]
        const yVal = row[v.yColumnName]
        if (xVal == null || yVal == null) continue
        let xNum: number
        if (xAxisMode === 'time') {
          xNum = timestampToMs(xVal, v.xType)
        } else {
          xNum = Number(xVal)
        }
        const yNum = Number(yVal)
        if (isNaN(xNum) || isNaN(yNum)) continue

        const point: ChartPoint = { x: xNum, y: yNum }
        if (v.colorColumnName && v.colorColumnKind) {
          const colorVal = row[v.colorColumnName]
          if (colorVal != null) {
            const css = cellColorToCss(colorVal, v.colorColumnKind)
            if (css !== null) point.color = css
          }
        }
        data.push(point)
      }
      // Sort by X ascending (uPlot requirement)
      data.sort((a, b) => a.x - b.x)
    }

    series.push({ label: seriesLabel, unit: seriesUnit, data })
  }

  // Handle categorical x-axis: build union label map
  if (xAxisMode === 'categorical') {
    const labelMap = new Map<string, number>()
    const xLabels: string[] = []

    // First pass: collect all unique labels from all tables
    for (let i = 0; i < tables.length; i++) {
      const { table } = tables[i]
      const v = validations[i]
      for (let r = 0; r < table.numRows; r++) {
        const row = table.get(r)
        if (!row) continue
        const xVal = row[v.xColumnName]
        if (xVal == null) continue
        const str = String(xVal)
        if (!labelMap.has(str)) {
          labelMap.set(str, xLabels.length)
          xLabels.push(str)
        }
      }
    }

    // Sort labels alphabetically for cross-series union
    xLabels.sort()
    // Rebuild map after sort
    labelMap.clear()
    xLabels.forEach((lbl, idx) => labelMap.set(lbl, idx))

    // Second pass: remap x values to indices (and decode color here)
    for (let i = 0; i < tables.length; i++) {
      const { table } = tables[i]
      const v = validations[i]
      const seriesData: ChartPoint[] = []

      for (let r = 0; r < table.numRows; r++) {
        const row = table.get(r)
        if (!row) continue
        const xVal = row[v.xColumnName]
        const yVal = row[v.yColumnName]
        if (xVal == null || yVal == null) continue
        const yNum = Number(yVal)
        if (isNaN(yNum)) continue
        const str = String(xVal)
        const idx = labelMap.get(str)
        if (idx == null) continue

        const point: ChartPoint = { x: idx, y: yNum }
        if (v.colorColumnName && v.colorColumnKind) {
          const colorVal = row[v.colorColumnName]
          if (colorVal != null) {
            const css = cellColorToCss(colorVal, v.colorColumnKind)
            if (css !== null) point.color = css
          }
        }
        seriesData.push(point)
      }

      series[i] = { ...series[i], data: seriesData }
    }

    return { ok: true, xAxisMode, xLabels, xColumnName, series }
  }

  return { ok: true, xAxisMode, xColumnName, series }
}

/**
 * Extract chart data from Arrow table (X and Y columns, plus optional 'color')
 */
export function extractChartData(table: Table):
  | {
      ok: true
      data: ChartPoint[]
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

  const { xType, xColumnName, yColumnName, colorColumnName, colorColumnKind } = validation
  const xAxisMode = detectXAxisMode(xType)

  const data: ChartPoint[] = []

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

      const point: ChartPoint = { x: labelMap.get(str)!, y: yNum }
      if (colorColumnName && colorColumnKind) {
        const colorVal = row[colorColumnName]
        if (colorVal != null) {
          const css = cellColorToCss(colorVal, colorColumnKind)
          if (css !== null) point.color = css
        }
      }
      data.push(point)
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

      const point: ChartPoint = { x: xNum, y: yNum }
      if (colorColumnName && colorColumnKind) {
        const colorVal = row[colorColumnName]
        if (colorVal != null) {
          const css = cellColorToCss(colorVal, colorColumnKind)
          if (css !== null) point.color = css
        }
      }
      data.push(point)
    }

    if (data.length === 0) {
      return { ok: false, error: 'No valid data points (all values are null)' }
    }

    // Time/numeric: sort by X ascending (uPlot requirement)
    data.sort((a, b) => a.x - b.x)

    return { ok: true, data, xAxisMode, xColumnName, yColumnName }
  }
}
