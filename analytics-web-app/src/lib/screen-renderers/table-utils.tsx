/**
 * Shared utilities for table rendering (TableRenderer and TableCell)
 */

/* eslint-disable react-refresh/only-export-components */

import { useMemo } from 'react'
import { ChevronUp, ChevronDown } from 'lucide-react'
import { DataType } from 'apache-arrow'
import Markdown from 'react-markdown'
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
// Column Override Types
// =============================================================================

/** Configuration for overriding how a column renders */
export interface ColumnOverride {
  /** Column name to override */
  column: string
  /** Markdown format string with $row.x or $row["x"] macros */
  format: string
}

// =============================================================================
// Macro Expansion
// =============================================================================

// Matches $row.columnName (dot notation for simple alphanumeric names)
const DOT_NOTATION_REGEX = /\$row\.(\w+)/g

// Matches $row["column-name"] or $row['column-name'] (bracket notation for any name)
const BRACKET_NOTATION_REGEX = /\$row\[["']([^"']+)["']\]/g

/**
 * Format a value for URL inclusion, handling timestamps as RFC3339.
 */
function formatValueForUrl(
  value: unknown,
  columnName: string,
  columnTypes: Map<string, DataType>
): string {
  if (value == null) return ''

  const dataType = columnTypes.get(columnName)
  if (dataType && isTimeType(dataType)) {
    // Format timestamps as RFC3339 (ISO 8601) for URL compatibility
    const date = timestampToDate(value, dataType)
    return date ? date.toISOString() : ''
  }

  return String(value)
}

/**
 * Expand $row macros using row data.
 * Supports two syntaxes:
 * - $row.columnName (dot notation for alphanumeric column names)
 * - $row["column-name"] (bracket notation for names with hyphens, spaces, etc.)
 *
 * When columnTypes is provided, timestamps are formatted as RFC3339.
 */
export function expandRowMacros(
  template: string,
  row: Record<string, unknown>,
  columnTypes?: Map<string, DataType>
): string {
  const types = columnTypes || new Map<string, DataType>()

  // First pass: bracket notation (handles special characters)
  let result = template.replace(BRACKET_NOTATION_REGEX, (_, columnName) => {
    const value = row[columnName]
    return formatValueForUrl(value, columnName, types)
  })

  // Second pass: dot notation (simple alphanumeric names)
  result = result.replace(DOT_NOTATION_REGEX, (_, columnName) => {
    const value = row[columnName]
    return formatValueForUrl(value, columnName, types)
  })

  return result
}

// =============================================================================
// Override Cell Component
// =============================================================================

interface OverrideCellProps {
  format: string
  row: Record<string, unknown>
  columns: TableColumn[]
}

/**
 * Render a column override: expand macros, then render markdown.
 * Timestamps are automatically formatted as RFC3339 for URL compatibility.
 */
export function OverrideCell({ format, row, columns }: OverrideCellProps) {
  // Build column type map for proper value formatting
  const columnTypes = useMemo(() => {
    const map = new Map<string, DataType>()
    for (const col of columns) {
      map.set(col.name, col.type)
    }
    return map
  }, [columns])

  const expanded = expandRowMacros(format, row, columnTypes)

  return (
    <Markdown
      components={{
        // Render links with proper attributes
        a: ({ href, children }) => (
          <a href={href} rel="noopener noreferrer" className="text-accent-link hover:underline">
            {children}
          </a>
        ),
        // Strip wrapper paragraph to keep content inline
        p: ({ children }) => <>{children}</>,
      }}
    >
      {expanded}
    </Markdown>
  )
}

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
// Table Body Component
// =============================================================================

/** Column definition for TableBody */
export interface TableColumn {
  name: string
  type: DataType
}

/** Data interface matching Arrow Table's row access pattern */
export interface TableData {
  numRows: number
  get(index: number): Record<string, unknown> | null
}

export interface TableBodyProps {
  data: TableData
  columns: TableColumn[]
  /** Use compact styling for notebook cells */
  compact?: boolean
  /** Column overrides for custom rendering */
  overrides?: ColumnOverride[]
}

export function TableBody({ data, columns, compact = false, overrides = [] }: TableBodyProps) {
  const rowClass = compact
    ? 'border-b border-theme-border hover:bg-app-card/50 transition-colors'
    : 'border-b border-theme-border hover:bg-app-card transition-colors'

  const cellClass = compact
    ? 'px-3 py-2 text-theme-text-primary font-mono truncate max-w-xs'
    : 'px-4 py-3 text-sm text-theme-text-primary font-mono truncate max-w-xs'

  // Build override lookup map
  const overrideMap = useMemo(() => {
    const map = new Map<string, string>()
    for (const o of overrides) {
      map.set(o.column, o.format)
    }
    return map
  }, [overrides])

  return (
    <tbody>
      {Array.from({ length: data.numRows }, (_, rowIdx) => {
        const row = data.get(rowIdx)
        if (!row) return null
        return (
          <tr key={rowIdx} className={rowClass}>
            {columns.map((col) => {
              const value = row[col.name]
              const override = overrideMap.get(col.name)

              // Use override renderer if configured for this column
              if (override) {
                return (
                  <td key={col.name} className={cellClass}>
                    <OverrideCell format={override} row={row} columns={columns} />
                  </td>
                )
              }

              const formatted = formatCell(value, col.type)
              // For non-compact mode, show raw value in tooltip (except binary which uses formatted)
              const tooltip =
                value != null
                  ? compact
                    ? formatted
                    : isBinaryType(col.type)
                      ? formatted
                      : String(value)
                  : undefined
              return (
                <td key={col.name} className={cellClass} title={tooltip}>
                  {formatted}
                </td>
              )
            })}
          </tr>
        )
      })}
    </tbody>
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
