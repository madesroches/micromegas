/**
 * Shared utilities for log rendering (LogRenderer and LogCell)
 */

import React from 'react'
import type { Field } from 'apache-arrow'
import { timestampToDate } from '@/lib/arrow-utils'
import { formatCell } from './table-utils'

// =============================================================================
// Constants
// =============================================================================

export const LEVEL_NAMES: Record<number, string> = {
  1: 'FATAL',
  2: 'ERROR',
  3: 'WARN',
  4: 'INFO',
  5: 'DEBUG',
  6: 'TRACE',
}

/** Known log columns in canonical display order */
const KNOWN_COLUMN_ORDER = ['time', 'level', 'target'] as const

export type KnownColumnName = (typeof KNOWN_COLUMN_ORDER)[number]

// =============================================================================
// Formatting
// =============================================================================

export function formatLocalTime(utcTime: unknown): string {
  if (!utcTime) return ''.padEnd(29)

  const date = timestampToDate(utcTime)
  if (!date) return ''.padEnd(29)

  let nanoseconds = '000000000'
  const str = String(utcTime)
  const nanoMatch = str.match(/\.(\d+)/)
  if (nanoMatch) {
    nanoseconds = nanoMatch[1].padEnd(9, '0').slice(0, 9)
  }

  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  const seconds = String(date.getSeconds()).padStart(2, '0')

  return `${year}-${month}-${day} ${hours}:${minutes}:${seconds}.${nanoseconds}`
}

export function getLevelColor(level: string): string {
  switch (level) {
    case 'FATAL':
      return 'text-accent-error-bright'
    case 'ERROR':
      return 'text-accent-error'
    case 'WARN':
      return 'text-accent-warning'
    case 'INFO':
      return 'text-accent-link'
    case 'DEBUG':
      return 'text-theme-text-secondary'
    case 'TRACE':
      return 'text-theme-text-muted'
    default:
      return 'text-theme-text-primary'
  }
}

export function formatLevelValue(levelValue: unknown): string {
  if (typeof levelValue === 'number') {
    return LEVEL_NAMES[levelValue] || 'UNKNOWN'
  }
  return String(levelValue ?? '')
}

// =============================================================================
// Column Classification
// =============================================================================

export interface LogColumn {
  name: string
  kind: KnownColumnName | 'generic'
  type: Field['type']
}

/**
 * Classify Arrow schema fields into log columns, preserving schema order.
 * Known columns (time, level, target) get special rendering via their
 * `kind` discriminant; all other columns (including `msg`) are tagged as
 * 'generic' and rendered as content-sized flex columns.
 */
export function classifyLogColumns(fields: Field[]): LogColumn[] {
  const knownSet = new Set<string>(KNOWN_COLUMN_ORDER)
  return fields.map((field) => ({
    name: field.name,
    kind: knownSet.has(field.name) ? (field.name as KnownColumnName) : 'generic',
    type: field.type,
  }))
}

// =============================================================================
// Rendering
// =============================================================================

const FLEX_CHAR_WIDTH_PX = 7.2
const MAX_FLEX_WIDTH_PX = 700
const MIN_FLEX_WIDTH_PX = 60
const MIN_LEVEL_WIDTH_PX = 40

export interface RenderLogColumnOptions {
  width?: number
  isLast?: boolean
  wrap?: boolean
}

function textCellClasses(wrap: boolean | undefined): string {
  return wrap ? 'whitespace-pre-wrap wrap-break-word' : 'truncate'
}

/** Formats a single column's value to the string shown/copied/measured for it,
 *  independent of any per-kind JSX styling (className/title/color). */
export function formatLogValue(col: LogColumn, value: unknown): string {
  switch (col.kind) {
    case 'time':
      return formatLocalTime(value)
    case 'level':
      return formatLevelValue(value)
    case 'target':
      return String(value ?? '')
    default:
      return formatCell(value, col.type)
  }
}

export function renderLogColumn(
  col: LogColumn,
  row: Record<string, unknown>,
  opts?: RenderLogColumnOptions,
): React.ReactNode {
  const value = row[col.name]
  const w = opts?.width
  const isLast = opts?.isLast
  const trailingMargin = opts?.isLast !== false ? 'mr-3' : ''
  // The last column has no divider/column after it, so let it fill the row width
  // that's actually available rather than stopping at (or reserving) its measured
  // width: it grows into any free space and shrinks down to MIN_FLEX_WIDTH_PX when
  // space is tight. Only when that minimum plus the other columns can't fit does
  // the row scroll horizontally — no hardcoded width ceiling is involved.
  const growStyle = { flexGrow: 1, flexShrink: 1, flexBasis: 0, minWidth: MIN_FLEX_WIDTH_PX }
  const widthStyle = isLast ? growStyle : w != null ? { width: w, minWidth: w, maxWidth: w } : undefined
  const wrapClasses = textCellClasses(opts?.wrap)
  switch (col.kind) {
    case 'time':
      return (
        <span
          className={`text-theme-text-muted ${trailingMargin} ${wrapClasses}`}
          style={widthStyle}
        >
          {formatLogValue(col, value)}
        </span>
      )
    case 'level': {
      const levelStr = formatLogValue(col, value)
      return (
        <span
          className={`${trailingMargin} font-semibold ${getLevelColor(levelStr)} ${wrapClasses}`}
          style={widthStyle}
        >
          {levelStr}
        </span>
      )
    }
    case 'target': {
      const targetStr = formatLogValue(col, value)
      return (
        <span
          className={`text-accent-highlight ${trailingMargin} ${wrapClasses}`}
          style={widthStyle}
          title={targetStr}
        >
          {targetStr}
        </span>
      )
    }
    default: {
      const formatted = formatLogValue(col, value)
      return (
        <span
          className={`text-theme-text-primary ${trailingMargin} ${wrapClasses}`}
          style={widthStyle ?? { minWidth: MIN_FLEX_WIDTH_PX, maxWidth: MAX_FLEX_WIDTH_PX }}
          title={formatted}
        >
          {formatted}
        </span>
      )
    }
  }
}

export function formatRowForCopy(columns: LogColumn[], row: Record<string, unknown>): string {
  return columns
    .map((col) => formatLogValue(col, row[col.name]))
    // Replace embedded tabs/newlines so a multi-line or tab-containing value
    // (e.g. a stack trace in `msg`) doesn't inject phantom rows/columns when
    // pasted into the tab-delimited output.
    .map((value) => value.replace(/[\t\r\n]+/g, ' '))
    .join('\t')
}

export function computeFlexWidths(
  table: { numRows: number; get(i: number): Record<string, unknown> | null | undefined } | null | undefined,
  columns: LogColumn[],
  rows: number[],
): Record<string, number> {
  if (!table || columns.length === 0) return {}
  const maxLens: Record<string, number> = {}
  for (const col of columns) maxLens[col.name] = 0
  for (const i of rows) {
    const row = table.get(i)
    for (const col of columns) {
      const formatted = formatLogValue(col, row?.[col.name])
      const len = formatted.length
      if (len > maxLens[col.name]) maxLens[col.name] = len
    }
  }
  const result: Record<string, number> = {}
  for (const col of columns) {
    const measured = Math.ceil(maxLens[col.name] * FLEX_CHAR_WIDTH_PX)
    switch (col.kind) {
      case 'time':
        // formatLocalTime always returns exactly 29 chars → 209px
        result[col.name] = Math.min(Math.max(measured, MIN_FLEX_WIDTH_PX), MAX_FLEX_WIDTH_PX)
        break
      case 'level':
        result[col.name] = Math.min(Math.max(measured, MIN_LEVEL_WIDTH_PX), MAX_FLEX_WIDTH_PX)
        break
      case 'target':
        result[col.name] = Math.min(Math.max(measured, MIN_FLEX_WIDTH_PX), 200)
        break
      default:
        result[col.name] = Math.min(Math.max(measured, MIN_FLEX_WIDTH_PX), MAX_FLEX_WIDTH_PX)
    }
  }
  return result
}

// =============================================================================
// Grouping consecutive repeated rows
// =============================================================================

export interface LogRowGroup {
  start: number
  end: number
}

/** `[start, end)` as an array of indices. */
export function range(start: number, end: number): number[] {
  const result: number[] = []
  for (let i = start; i < end; i++) result.push(i)
  return result
}

/** One group per row — the no-collapsing case. */
export function singletonGroups(n: number): LogRowGroup[] {
  const groups: LogRowGroup[] = []
  for (let i = 0; i < n; i++) groups.push({ start: i, end: i + 1 })
  return groups
}

/** `===` covers strings/numbers/bigints/booleans/null directly. A formatted-string
 *  fallback only kicks in for non-primitives (Arrow struct/list/`Uint8Array`/Date),
 *  where "identical" means "renders identically" — which is what the user sees. */
function logValuesEqual(col: LogColumn, a: unknown, b: unknown): boolean {
  if (a === b) return true
  const isPrimitive = (v: unknown) => v === null || v === undefined || typeof v !== 'object'
  if (isPrimitive(a) || isPrimitive(b)) return false
  return formatLogValue(col, a) === formatLogValue(col, b)
}

interface LogGroupableTable {
  numRows: number
  getChild(name: string): { get(i: number): unknown } | null
}

/**
 * Splits `table`'s rows into maximal runs of consecutive rows equal on every
 * column not in `ignore`. Reads through `table.getChild(col.name)` for compared
 * columns only, so it never builds a row proxy: O(rows × compared columns)
 * with an early exit on the first mismatching column.
 */
export function groupConsecutiveRows(
  table: LogGroupableTable | null | undefined,
  columns: LogColumn[],
  ignore: ReadonlySet<string>,
): LogRowGroup[] {
  if (!table || table.numRows === 0) return []
  const numRows = table.numRows
  const compared = columns.filter((col) => !ignore.has(col.name))
  // If every column is ignored, any two rows would compare equal — fall back
  // to one row per group instead of collapsing the whole result into one line.
  if (compared.length === 0) return singletonGroups(numRows)
  const vectors = compared.map((col) => ({ col, vec: table.getChild(col.name) }))
  const groups: LogRowGroup[] = []
  let start = 0
  for (let i = 1; i < numRows; i++) {
    const matchesGroupStart = vectors.every(({ col, vec }) =>
      logValuesEqual(col, vec?.get(start), vec?.get(i)),
    )
    if (!matchesGroupStart) {
      groups.push({ start, end: i })
      start = i
    }
  }
  groups.push({ start, end: numRows })
  return groups
}

