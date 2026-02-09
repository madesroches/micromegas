/**
 * Shared utilities for table rendering (TableRenderer and TableCell)
 */

/* eslint-disable react-refresh/only-export-components */

import { useMemo } from 'react'
import * as ContextMenu from '@radix-ui/react-context-menu'
import { ChevronUp, ChevronDown, EyeOff, ArrowUpNarrowWide, ArrowDownNarrowWide, X } from 'lucide-react'
import { DataType } from 'apache-arrow'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { formatTimestamp, formatDurationMs } from '@/lib/time-range'
import {
  timestampToDate,
  isTimeType,
  isNumericType,
  isBinaryType,
  isDurationType,
  durationToMs,
} from '@/lib/arrow-utils'
import type { VariableValue } from './notebook-types'
import { getVariableString } from './notebook-types'

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
 * Extract all column names referenced in a format template.
 * Returns unique column names from both $row.name and $row["name"] syntaxes.
 */
export function extractMacroColumns(template: string): string[] {
  const columns = new Set<string>()

  // Extract bracket notation references
  let match: RegExpExecArray | null
  const bracketRegex = /\$row\[["']([^"']+)["']\]/g
  while ((match = bracketRegex.exec(template)) !== null) {
    columns.add(match[1])
  }

  // Extract dot notation references
  const dotRegex = /\$row\.(\w+)/g
  while ((match = dotRegex.exec(template)) !== null) {
    columns.add(match[1])
  }

  return Array.from(columns)
}

// Built-in macros that are always valid
const BUILTIN_MACROS = new Set(['row', 'begin', 'end'])

/**
 * Find unknown macro patterns like $name that aren't known variables.
 * Returns array of unknown macro strings found.
 *
 * @param template - The format template to check
 * @param availableVariables - Known variable names (without $)
 */
export function findUnknownMacros(template: string, availableVariables: string[]): string[] {
  const unknown: string[] = []
  const knownVars = new Set(availableVariables)

  // Match $ followed by a word
  const macroRegex = /\$(\w+)/g
  let match: RegExpExecArray | null
  while ((match = macroRegex.exec(template)) !== null) {
    const name = match[1]
    // Skip if it's a built-in macro or a known variable
    if (!BUILTIN_MACROS.has(name) && !knownVars.has(name)) {
      unknown.push(match[0]) // Include the $ in the result (e.g., "$missing")
    }
  }

  return unknown
}

/** Validation result for format macros */
export interface FormatValidation {
  /** Column names referenced but not available */
  missingColumns: string[]
  /** Unknown macros (e.g., $name where name is not a known variable) */
  unknownMacros: string[]
}

/**
 * Validate a format template against available columns and variables.
 * Returns missing columns and unknown macros.
 *
 * @param template - The format template to validate
 * @param availableColumns - Column names from the query result
 * @param availableVariables - Variable names from notebook (optional)
 */
export function validateFormatMacros(
  template: string,
  availableColumns: string[],
  availableVariables: string[] = []
): FormatValidation {
  const referenced = extractMacroColumns(template)
  const available = new Set(availableColumns)
  const missingColumns = referenced.filter((col) => !available.has(col))
  const unknownMacros = findUnknownMacros(template, availableVariables)

  return { missingColumns, unknownMacros }
}

/**
 * Expand variable macros like $search, $metric, etc.
 * Sorts by name length descending to avoid partial matches ($metric vs $metric_name).
 */
export function expandVariableMacros(
  template: string,
  variables: Record<string, VariableValue>
): string {
  let result = template
  // Sort by name length descending to avoid partial matches
  const sortedVars = Object.entries(variables).sort((a, b) => b[0].length - a[0].length)
  for (const [name, value] of sortedVars) {
    const regex = new RegExp(`\\$${name}\\b`, 'g')
    // Use getVariableString to handle both simple strings and multi-column objects
    result = result.replace(regex, getVariableString(value))
  }
  return result
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
  /** Notebook variables for macro expansion */
  variables?: Record<string, VariableValue>
}

/**
 * Render a column override: expand macros, then render markdown.
 * Expands both notebook variables ($name) and row data ($row.column).
 * Timestamps are automatically formatted as RFC3339 for URL compatibility.
 */
export function OverrideCell({ format, row, columns, variables = {} }: OverrideCellProps) {
  // Build column type map for proper value formatting
  const columnTypes = useMemo(() => {
    const map = new Map<string, DataType>()
    for (const col of columns) {
      map.set(col.name, col.type)
    }
    return map
  }, [columns])

  // First expand notebook variables, then row macros
  const withVariables = expandVariableMacros(format, variables)
  const expanded = expandRowMacros(withVariables, row, columnTypes)

  return (
    <div className="prose prose-invert prose-sm max-w-none prose-headings:text-theme-text-primary prose-headings:my-0 prose-p:text-theme-text-secondary prose-p:my-0 prose-a:text-accent-link prose-strong:text-theme-text-primary prose-code:text-accent-highlight prose-code:bg-app-card prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-code:before:content-none prose-code:after:content-none prose-li:text-theme-text-secondary">
      <Markdown
        remarkPlugins={[remarkGfm]}
        components={{
          // Render links with proper security attributes
          a: ({ href, children }) => (
            <a href={href} rel="noopener noreferrer">
              {children}
            </a>
          ),
        }}
      >
        {expanded}
      </Markdown>
    </div>
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
  /** Set sort to ascending unconditionally */
  onSortAsc?: (columnName: string) => void
  /** Set sort to descending unconditionally */
  onSortDesc?: (columnName: string) => void
  /** Use compact padding for notebook cells */
  compact?: boolean
  /** When provided, right-click opens a context menu with a "Hide column" option */
  onHide?: (columnName: string) => void
}

export function SortHeader({
  columnName,
  children,
  sortColumn,
  sortDirection,
  onSort,
  onSortAsc,
  onSortDesc,
  compact = false,
  onHide,
}: SortHeaderProps) {
  const isActive = sortColumn === columnName
  const showAsc = isActive && sortDirection === 'asc'
  const showDesc = isActive && sortDirection === 'desc'

  const padding = compact ? 'px-3 py-2' : 'px-4 py-3'
  const hoverBg = compact ? 'hover:bg-app-card/50' : 'hover:bg-app-card'

  const thContent = (
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
  )

  const thClass = `${padding} text-left text-xs font-semibold uppercase tracking-wider cursor-pointer select-none transition-colors ${
    isActive
      ? 'text-theme-text-primary bg-app-card'
      : `text-theme-text-muted hover:text-theme-text-secondary ${hoverBg}`
  }`

  if (!onHide) {
    return (
      <th onClick={() => onSort(columnName)} className={thClass}>
        {thContent}
      </th>
    )
  }

  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>
        <th onClick={() => onSort(columnName)} className={thClass}>
          {thContent}
        </th>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content className="min-w-[180px] bg-app-panel border border-theme-border rounded-md shadow-lg py-1 z-50">
          <ContextMenu.Item
            onSelect={() => (onSortAsc ?? onSort)(columnName)}
            className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none"
          >
            <ArrowUpNarrowWide className="w-4 h-4 text-theme-text-secondary" />
            Sort Ascending
          </ContextMenu.Item>
          <ContextMenu.Item
            onSelect={() => (onSortDesc ?? onSort)(columnName)}
            className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none"
          >
            <ArrowDownNarrowWide className="w-4 h-4 text-theme-text-secondary" />
            Sort Descending
          </ContextMenu.Item>
          <ContextMenu.Separator className="h-px bg-theme-border my-1" />
          <ContextMenu.Item
            onSelect={() => onHide(columnName)}
            className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-secondary hover:bg-theme-border/50 cursor-pointer outline-none"
          >
            <EyeOff className="w-4 h-4" />
            Hide Column
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
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
  /** Notebook variables for macro expansion in overrides */
  variables?: Record<string, VariableValue>
}

export function TableBody({ data, columns, compact = false, overrides = [], variables = {} }: TableBodyProps) {
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
                    <OverrideCell format={override} row={row} columns={columns} variables={variables} />
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
// Hidden Columns Bar
// =============================================================================

export interface HiddenColumnsBarProps {
  hiddenColumns: string[]
  onRestore: (columnName: string) => void
  /** Restore all hidden columns in a single update */
  onRestoreAll?: () => void
  /** Use compact styling for notebook cells */
  compact?: boolean
}

export function HiddenColumnsBar({ hiddenColumns, onRestore, onRestoreAll, compact = false }: HiddenColumnsBarProps) {

  if (hiddenColumns.length === 0) return null

  const iconSize = compact ? 'w-3 h-3' : 'w-3.5 h-3.5'
  const textSize = compact ? 'text-[10px]' : 'text-xs'
  const padding = compact ? 'px-3 py-1' : 'px-4 py-1.5'
  const gap = compact ? 'gap-1.5' : 'gap-2'
  const pillPadding = compact ? 'px-1.5 py-0' : 'px-2 py-0.5'

  return (
    <div
      className={`flex items-center ${gap} ${padding} bg-accent-link/[0.08] border-b border-theme-border flex-wrap`}
    >
      <EyeOff className={`${iconSize} text-theme-text-muted flex-shrink-0`} />
      <span className={`${textSize} text-theme-text-muted`}>Hidden:</span>
      {hiddenColumns.map((col) => (
        <button
          key={col}
          onClick={() => onRestore(col)}
          className={`inline-flex items-center gap-1 ${pillPadding} ${textSize} bg-accent-link/15 text-accent-link border border-accent-link/30 rounded-full hover:bg-accent-link/25 hover:border-accent-link/50 transition-colors`}
        >
          {col}
          <X className="w-3 h-3 opacity-70" />
        </button>
      ))}
      {hiddenColumns.length > 1 && onRestoreAll && (
        <button
          onClick={onRestoreAll}
          className={`${textSize} text-accent-link hover:text-accent-link-hover underline underline-offset-2 transition-colors`}
        >
          Show all
        </button>
      )}
    </div>
  )
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
