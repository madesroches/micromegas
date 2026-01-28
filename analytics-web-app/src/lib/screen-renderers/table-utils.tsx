/**
 * Shared utilities for table rendering (TableRenderer and TableCell)
 */

/* eslint-disable react-refresh/only-export-components */

import { ChevronUp, ChevronDown } from 'lucide-react'
import { DataType } from 'apache-arrow'
import { formatTimestamp, formatDurationMs } from '@/lib/time-range'
import {
  timestampToDate,
  isTimeType,
  isNumericType,
  isBinaryType,
  isDurationType,
  durationToMs,
} from '@/lib/arrow-utils'

// =============================================================================
// Sort Header Component
// =============================================================================

export interface SortHeaderProps {
  columnName: string
  children: React.ReactNode
  sortColumn?: string
  sortDirection?: 'asc' | 'desc'
  onSort: (columnName: string) => void
  /** Use compact padding for notebook cells */
  compact?: boolean
}

export function SortHeader({
  columnName,
  children,
  sortColumn,
  sortDirection,
  onSort,
  compact = false,
}: SortHeaderProps) {
  const isActive = sortColumn === columnName
  const showAsc = isActive && sortDirection === 'asc'
  const showDesc = isActive && sortDirection === 'desc'

  const padding = compact ? 'px-3 py-2' : 'px-4 py-3'
  const hoverBg = compact ? 'hover:bg-app-card/50' : 'hover:bg-app-card'

  return (
    <th
      onClick={() => onSort(columnName)}
      className={`${padding} text-left text-xs font-semibold uppercase tracking-wider cursor-pointer select-none transition-colors ${
        isActive
          ? 'text-theme-text-primary bg-app-card'
          : `text-theme-text-muted hover:text-theme-text-secondary ${hoverBg}`
      }`}
    >
      <div className="flex items-center gap-1">
        <span className="truncate">{children}</span>
        {isActive && (
          <span className="text-accent-link flex-shrink-0">
            {showAsc ? (
              <ChevronUp className="w-3 h-3" />
            ) : showDesc ? (
              <ChevronDown className="w-3 h-3" />
            ) : null}
          </span>
        )}
      </div>
    </th>
  )
}

// =============================================================================
// Cell Formatting
// =============================================================================

/**
 * Format a cell value based on its Arrow DataType.
 */
export function formatCell(value: unknown, dataType: DataType): string {
  if (value === null || value === undefined) return '-'

  if (isTimeType(dataType)) {
    const date = timestampToDate(value, dataType)
    return date ? formatTimestamp(date) : '-'
  }

  if (isDurationType(dataType)) {
    const ms = durationToMs(value, dataType)
    return formatDurationMs(ms)
  }

  if (isNumericType(dataType)) {
    if (typeof value === 'number') {
      return value.toLocaleString()
    }
    if (typeof value === 'bigint') {
      return value.toLocaleString()
    }
    return String(value)
  }

  if (DataType.isBool(dataType)) {
    return value ? 'true' : 'false'
  }

  // Binary data: display as ASCII preview with length
  if (isBinaryType(dataType)) {
    const bytes = value instanceof Uint8Array ? value : Array.isArray(value) ? value : null
    if (bytes) {
      const previewLen = Math.min(bytes.length, 32)
      let preview = ''
      for (let i = 0; i < previewLen; i++) {
        const b = bytes[i]
        // Printable ASCII range: 32-126
        preview += b >= 32 && b <= 126 ? String.fromCharCode(b) : '.'
      }
      const suffix = bytes.length > previewLen ? '...' : ''
      return `${preview}${suffix} (${bytes.length})`
    }
  }

  return String(value)
}

// =============================================================================
// Sort Utilities
// =============================================================================

/**
 * Build ORDER BY clause from sort state.
 */
export function buildOrderByClause(
  sortColumn: string | undefined,
  sortDirection: 'asc' | 'desc' | undefined
): string {
  if (sortColumn && sortDirection) {
    return `ORDER BY ${sortColumn} ${sortDirection.toUpperCase()}`
  }
  return ''
}

/**
 * Compute next sort state using three-state cycling: none -> ASC -> DESC -> none
 */
export function getNextSortState(
  columnName: string,
  currentSortColumn: string | undefined,
  currentSortDirection: 'asc' | 'desc' | undefined
): { sortColumn: string | undefined; sortDirection: 'asc' | 'desc' | undefined } {
  if (currentSortColumn !== columnName) {
    // New column: start with ASC
    return { sortColumn: columnName, sortDirection: 'asc' }
  } else if (currentSortDirection === 'asc') {
    // ASC -> DESC
    return { sortColumn: columnName, sortDirection: 'desc' }
  } else {
    // DESC -> no sort (clear)
    return { sortColumn: undefined, sortDirection: undefined }
  }
}
