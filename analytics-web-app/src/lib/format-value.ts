/**
 * Shared adaptive unit formatter.
 *
 * `formatValueWithUnit` picks the best display unit for an individual value
 * (e.g. 3678630912 bytes -> "3.4 GB"). Used by both the chart cell (per-stat
 * formatting in `XYChart`) and the template `format_value` function.
 */

import { isTimeUnit, formatTimeValue } from './time-units'
import {
  normalizeUnit,
  isSizeUnit,
  getAdaptiveSizeUnit,
  isBitUnit,
  getAdaptiveBitUnit,
} from './units'

function formatNonTime(value: number, rawUnit: string): string {
  const unit = normalizeUnit(rawUnit)

  if (isSizeUnit(unit)) {
    const adaptive = getAdaptiveSizeUnit(value, unit)
    const displayValue = value * adaptive.conversionFactor
    const decimals = adaptive.unit === 'bytes' ? 0 : 1
    return displayValue.toFixed(decimals) + ' ' + adaptive.abbrev
  }

  if (isBitUnit(unit)) {
    const adaptive = getAdaptiveBitUnit(value, unit)
    const displayValue = value * adaptive.conversionFactor
    const decimals = adaptive.unit === 'bits' ? 0 : 1
    return displayValue.toFixed(decimals) + ' ' + adaptive.abbrev
  }

  if (unit === 'percent') return value.toFixed(1) + '%'
  if (unit === 'degrees') return value.toFixed(1) + '°'
  if (unit === 'boolean') return value !== 0 ? 'true' : 'false'

  return rawUnit ? `${value.toLocaleString()} ${rawUnit}` : value.toLocaleString()
}

/**
 * Format a numeric value with adaptive unit scaling, picking the best display
 * unit for this individual value.
 */
export function formatValueWithUnit(value: number, rawUnit: string): string {
  if (isTimeUnit(rawUnit)) {
    return formatTimeValue(value, rawUnit, false)
  }
  return formatNonTime(value, rawUnit)
}
