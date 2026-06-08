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

export interface RenderLogColumnOptions {
  width?: number
}

export function renderLogColumn(
  col: LogColumn,
  row: Record<string, unknown>,
  opts?: RenderLogColumnOptions,
): React.ReactNode {
  const value = row[col.name]
  switch (col.kind) {
    case 'time':
      return (
        <span className="text-theme-text-muted mr-3 w-[188px] min-w-[188px] whitespace-nowrap">
          {formatLocalTime(value)}
        </span>
      )
    case 'level': {
      const levelStr = formatLevelValue(value)
      return (
        <span className={`w-[38px] min-w-[38px] mr-3 font-semibold ${getLevelColor(levelStr)}`}>
          {levelStr}
        </span>
      )
    }
    case 'target': {
      const targetStr = String(value ?? '')
      return (
        <span
          className="text-accent-highlight mr-3 w-[200px] min-w-[200px] truncate"
          title={targetStr}
        >
          {targetStr}
        </span>
      )
    }
    default: {
      const formatted = formatCell(value, col.type)
      const w = opts?.width
      return (
        <span
          className="text-theme-text-primary mr-3 truncate"
          style={w != null ? { width: w, minWidth: w, maxWidth: w } : { minWidth: MIN_FLEX_WIDTH_PX, maxWidth: MAX_FLEX_WIDTH_PX }}
          title={formatted}
        >
          {formatted}
        </span>
      )
    }
  }
}

export function computeFlexWidths(
  table: { numRows: number; get(i: number): Record<string, unknown> | null | undefined } | null | undefined,
  columns: LogColumn[],
  startRow: number,
  endRow: number,
): Record<string, number> {
  const flexCols = columns.filter((c) => c.kind === 'generic')
  if (!table || flexCols.length === 0) return {}
  const maxLens: Record<string, number> = {}
  for (const col of flexCols) maxLens[col.name] = 0
  for (let i = startRow; i < endRow; i++) {
    const row = table.get(i)
    for (const col of flexCols) {
      const len = formatCell(row?.[col.name], col.type).length
      if (len > maxLens[col.name]) maxLens[col.name] = len
    }
  }
  const result: Record<string, number> = {}
  for (const col of flexCols) {
    result[col.name] = Math.min(
      Math.max(Math.ceil(maxLens[col.name] * FLEX_CHAR_WIDTH_PX), MIN_FLEX_WIDTH_PX),
      MAX_FLEX_WIDTH_PX,
    )
  }
  return result
}
