/**
 * Shared utilities for log rendering (LogRenderer and LogCell)
 */

import React from 'react'
import * as ContextMenu from '@radix-ui/react-context-menu'
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
}

export function renderLogColumn(
  col: LogColumn,
  row: Record<string, unknown>,
  opts?: RenderLogColumnOptions,
): React.ReactNode {
  const value = row[col.name]
  const w = opts?.width
  const trailingMargin = opts?.isLast !== false ? 'mr-3' : ''
  const widthStyle = w != null ? { width: w, minWidth: w, maxWidth: w } : undefined
  switch (col.kind) {
    case 'time':
      return (
        <span
          className={`text-theme-text-muted ${trailingMargin} whitespace-nowrap`}
          style={widthStyle}
        >
          {formatLocalTime(value)}
        </span>
      )
    case 'level': {
      const levelStr = formatLevelValue(value)
      return (
        <span
          className={`${trailingMargin} font-semibold ${getLevelColor(levelStr)}`}
          style={widthStyle}
        >
          {levelStr}
        </span>
      )
    }
    case 'target': {
      const targetStr = String(value ?? '')
      return (
        <span
          className={`text-accent-highlight ${trailingMargin} truncate`}
          style={widthStyle}
          title={targetStr}
        >
          {targetStr}
        </span>
      )
    }
    default: {
      const formatted = formatCell(value, col.type)
      return (
        <span
          className={`text-theme-text-primary ${trailingMargin} truncate`}
          style={
            w != null
              ? { width: w, minWidth: w, maxWidth: w }
              : { minWidth: MIN_FLEX_WIDTH_PX, maxWidth: MAX_FLEX_WIDTH_PX }
          }
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
  if (!table || columns.length === 0) return {}
  const maxLens: Record<string, number> = {}
  for (const col of columns) maxLens[col.name] = 0
  for (let i = startRow; i < endRow; i++) {
    const row = table.get(i)
    for (const col of columns) {
      let formatted: string
      switch (col.kind) {
        case 'time':
          formatted = formatLocalTime(row?.[col.name])
          break
        case 'level':
          formatted = formatLevelValue(row?.[col.name])
          break
        case 'target':
          formatted = String(row?.[col.name] ?? '')
          break
        default:
          formatted = formatCell(row?.[col.name], col.type)
      }
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
// LogDivider
// =============================================================================

export interface LogDividerProps {
  col: string
  pinned: boolean
  hovered: boolean
  onMouseDown: (e: React.MouseEvent) => void
  onContextMenu: (e: React.MouseEvent) => void
  onMouseEnter: () => void
  onMouseLeave: () => void
  onResetToAuto: () => void
  onResetAll: () => void
}

export function LogDivider({
  col,
  pinned,
  hovered,
  onMouseDown,
  onContextMenu,
  onMouseEnter,
  onMouseLeave,
  onResetToAuto,
  onResetAll,
}: LogDividerProps) {
  const lineColor = hovered
    ? '#3b82f6'
    : pinned
      ? '#f59e0b'
      : 'rgba(255,255,255,0.12)'

  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>
        <span
          data-col={col}
          onMouseDown={onMouseDown}
          onContextMenu={onContextMenu}
          onMouseEnter={onMouseEnter}
          onMouseLeave={onMouseLeave}
          style={{
            display: 'inline-flex',
            alignSelf: 'stretch',
            width: 5,
            minWidth: 5,
            cursor: 'col-resize',
            alignItems: 'center',
            justifyContent: 'center',
            flexShrink: 0,
          }}
        >
          <span
            style={{
              display: 'block',
              width: 1,
              height: '100%',
              backgroundColor: lineColor,
              transition: 'background-color 0.1s',
            }}
          />
        </span>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content
          className="min-w-[160px] bg-app-panel border border-theme-border rounded-md shadow-lg py-1 z-50"
        >
          <ContextMenu.Item
            onSelect={onResetToAuto}
            disabled={!pinned}
            className="flex items-center px-3 py-1.5 text-xs text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none data-[disabled]:opacity-40 data-[disabled]:cursor-default"
          >
            Reset to auto
          </ContextMenu.Item>
          <ContextMenu.Item
            onSelect={onResetAll}
            className="flex items-center px-3 py-1.5 text-xs text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none"
          >
            Reset all columns
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  )
}
