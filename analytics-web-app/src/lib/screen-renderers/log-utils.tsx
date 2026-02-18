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
const KNOWN_COLUMN_ORDER = ['time', 'level', 'target', 'msg'] as const

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
 * Known columns (time, level, target, msg) get special rendering via their
 * `kind` discriminant; all other columns are tagged as 'generic'.
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

export function renderLogColumn(col: LogColumn, row: Record<string, unknown>): React.ReactNode {
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
    case 'msg':
      return (
        <span className="text-theme-text-primary flex-1 break-words">{String(value ?? '')}</span>
      )
    default: {
      const formatted = formatCell(value, col.type)
      return (
        <span className="text-theme-text-secondary mr-3 truncate max-w-[200px]" title={formatted}>
          {formatted}
        </span>
      )
    }
  }
}
