import { Table } from 'apache-arrow'

// ============================================================================
// Variable Value Types
// ============================================================================

/**
 * A variable value can be:
 * - A simple string (single column variable or text/number input)
 * - An object mapping column names to string values (multi-column variable)
 */
export type VariableValue = string | Record<string, string>

/**
 * Gets the string representation of a variable value.
 * For multi-column values, returns the JSON representation with sorted keys.
 */
export function getVariableString(value: VariableValue): string {
  if (typeof value === 'string') return value
  // Sort keys for consistent output
  const sorted = Object.keys(value).sort().reduce((acc, key) => {
    acc[key] = value[key]
    return acc
  }, {} as Record<string, string>)
  return JSON.stringify(sorted)
}

/**
 * Checks if a variable value is a multi-column object.
 */
export function isMultiColumnValue(value: VariableValue): value is Record<string, string> {
  return typeof value !== 'string'
}

/**
 * Checks if two variable values are equal.
 */
export function variableValuesEqual(a: VariableValue, b: VariableValue): boolean {
  if (typeof a === 'string' && typeof b === 'string') {
    return a === b
  }
  if (typeof a === 'string' || typeof b === 'string') {
    return false
  }
  // Both are objects - compare keys and values
  const keysA = Object.keys(a)
  const keysB = Object.keys(b)
  if (keysA.length !== keysB.length) return false
  return keysA.every(key => key in b && a[key] === b[key])
}

/** Prefix used to identify multi-column values in URL storage */
const MULTI_COL_PREFIX = 'mcol:'

/**
 * Serializes a variable value for URL storage.
 * Simple strings are stored as-is, objects are prefixed with 'mcol:' and JSON-encoded.
 * Keys are sorted to ensure consistent serialization regardless of object key order.
 */
export function serializeVariableValue(value: VariableValue): string {
  if (typeof value === 'string') return value
  // Sort keys for consistent serialization (object key order varies)
  const sorted = Object.keys(value).sort().reduce((acc, key) => {
    acc[key] = value[key]
    return acc
  }, {} as Record<string, string>)
  return MULTI_COL_PREFIX + JSON.stringify(sorted)
}

/**
 * Deserializes a variable value from URL storage.
 * Values prefixed with 'mcol:' are parsed as JSON objects, others returned as strings.
 */
export function deserializeVariableValue(str: string): VariableValue {
  if (str.startsWith(MULTI_COL_PREFIX)) {
    try {
      const parsed = JSON.parse(str.slice(MULTI_COL_PREFIX.length))
      if (typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)) {
        // Verify all values are strings
        const isValid = Object.values(parsed).every((v) => typeof v === 'string')
        if (isValid) {
          return parsed as Record<string, string>
        }
      }
    } catch {
      // Invalid JSON after prefix, return original string
    }
  }
  return str
}

// ============================================================================
// Cell Configuration Types
// ============================================================================

// Note: CellType is defined here and re-exported from cell-registry.ts
// This avoids circular dependencies while keeping the types together
export type CellType = 'table' | 'chart' | 'log' | 'markdown' | 'variable' | 'propertytimeline' | 'swimlane'

export type CellStatus = 'idle' | 'loading' | 'success' | 'error' | 'blocked'

export interface CellConfigBase {
  name: string
  type: CellType
  layout: { height: number; collapsed?: boolean }
}

export interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log' | 'propertytimeline' | 'swimlane'
  sql: string
  options?: Record<string, unknown>
}

export interface MarkdownCellConfig extends CellConfigBase {
  type: 'markdown'
  content: string
}

export interface VariableCellConfig extends CellConfigBase {
  type: 'variable'
  variableType: 'combobox' | 'text' | 'number'
  sql?: string
  defaultValue?: VariableValue
}

export type CellConfig = QueryCellConfig | MarkdownCellConfig | VariableCellConfig

// ============================================================================
// Cell Execution State
// ============================================================================

export interface CellState {
  status: CellStatus
  error?: string
  data: Table | null
  /** For variable cells (combobox): options loaded from query */
  variableOptions?: { label: string; value: VariableValue }[]
}

// ============================================================================
// Notebook Configuration
// ============================================================================

export interface NotebookConfig {
  cells: CellConfig[]
  refreshInterval?: number
  timeRangeFrom?: string
  timeRangeTo?: string
}
