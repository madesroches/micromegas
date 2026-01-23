import { Table } from 'apache-arrow'

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
  variableOptions?: { label: string; value: string }[]
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
