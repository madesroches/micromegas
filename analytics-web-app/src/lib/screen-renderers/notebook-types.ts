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
 * For multi-column values, returns the JSON representation.
 */
export function getVariableString(value: VariableValue): string {
  if (typeof value === 'string') return value
  return JSON.stringify(value)
}

/**
 * Checks if a variable value is a multi-column object.
 */
export function isMultiColumnValue(value: VariableValue): value is Record<string, string> {
  return typeof value !== 'string'
}

/**
 * Serializes a variable value for URL storage.
 * Simple strings are stored as-is, objects are JSON-encoded.
 */
export function serializeVariableValue(value: VariableValue): string {
  if (typeof value === 'string') return value
  return JSON.stringify(value)
}

/**
 * Deserializes a variable value from URL storage.
 * Attempts to parse as JSON; if it fails, returns as simple string.
 */
export function deserializeVariableValue(str: string): VariableValue {
  // Try to parse as JSON object
  if (str.startsWith('{')) {
    try {
      const parsed = JSON.parse(str)
      if (typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)) {
        // Verify all values are strings
        const isValid = Object.values(parsed).every((v) => typeof v === 'string')
        if (isValid) {
          return parsed as Record<string, string>
        }
      }
    } catch {
      // Not valid JSON, return as string
    }
  }
  return str
}

// ============================================================================
// Cell Configuration Types
// ============================================================================

// Note: CellType is defined here and re-exported from cell-registry.ts
// This avoids circular dependencies while keeping the types together
export type CellType = 'table' | 'chart' | 'log' | 'markdown' | 'variable'

export type CellStatus = 'idle' | 'loading' | 'success' | 'error' | 'blocked'

export interface CellConfigBase {
  name: string
  type: CellType
  layout: { height: number; collapsed?: boolean }
}

export interface QueryCellConfig extends CellConfigBase {
  type: 'table' | 'chart' | 'log'
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
  defaultValue?: string
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
