/**
 * Shared utilities for log rendering (LogRenderer and LogCell)
 */

import type { Field } from 'apache-arrow'
import { timestampToDate } from '@/lib/arrow-utils'

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
 * Classify Arrow schema fields into ordered log columns.
 * Known columns (time, level, target, msg) appear first in canonical order,
 * followed by extra columns in their original schema order.
 */
export function classifyLogColumns(fields: Field[]): LogColumn[] {
  const fieldMap = new Map(fields.map((f) => [f.name, f]))
  const columns: LogColumn[] = []

  // Known columns in canonical order (only if present)
  for (const name of KNOWN_COLUMN_ORDER) {
    const field = fieldMap.get(name)
    if (field) {
      columns.push({ name, kind: name, type: field.type })
    }
  }

  // Extra columns in schema order
  const knownSet = new Set<string>(KNOWN_COLUMN_ORDER)
  for (const field of fields) {
    if (!knownSet.has(field.name)) {
      columns.push({ name: field.name, kind: 'generic', type: field.type })
    }
  }

  return columns
}
