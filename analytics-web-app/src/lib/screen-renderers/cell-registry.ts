import { ComponentType } from 'react'
import { Table } from 'apache-arrow'

/**
 * Cell type names supported in notebooks.
 */
export type CellType = 'table' | 'chart' | 'log' | 'markdown' | 'variable'

/**
 * Cell execution state.
 */
export type CellStatus = 'idle' | 'loading' | 'success' | 'error' | 'blocked'

/**
 * Props passed to all cell renderers.
 */
export interface CellRendererProps {
  /** Cell name (unique within notebook) */
  name: string
  /** SQL query for this cell (undefined for markdown cells) */
  sql?: string
  /** Cell-specific options (e.g., chart options) */
  options?: Record<string, unknown>
  /** Query result data */
  data: Table | null
  /** Current execution status */
  status: CellStatus
  /** Error message if status is 'error' */
  error?: string
  /** Time range for queries */
  timeRange: { begin: string; end: string }
  /** Available variables (from variable cells above) */
  variables: Record<string, string>
  /** Whether the cell is in edit mode */
  isEditing: boolean
  /** Request to run this cell */
  onRun: () => void
  /** Update cell SQL */
  onSqlChange: (sql: string) => void
  /** Update cell options */
  onOptionsChange: (options: Record<string, unknown>) => void
  /** For markdown cells: content to display */
  content?: string
  /** For markdown cells: update content */
  onContentChange?: (content: string) => void
  /** For variable cells: current value */
  value?: string
  /** For variable cells: update value */
  onValueChange?: (value: string) => void
  /** For variable cells: variable type */
  variableType?: 'combobox' | 'text' | 'number'
  /** For variable cells (combobox): available options from query */
  variableOptions?: { label: string; value: string }[]
}

// Registry populated by cell renderer imports
export const CELL_RENDERERS: Record<string, ComponentType<CellRendererProps>> = {}

/**
 * Register a renderer for a cell type.
 * Called by each cell renderer module on import.
 */
export function registerCellRenderer(
  typeName: CellType,
  component: ComponentType<CellRendererProps>
): void {
  CELL_RENDERERS[typeName] = component
}

/**
 * Get a renderer for a cell type.
 * Returns undefined if no renderer is registered.
 */
export function getCellRenderer(
  typeName: string
): ComponentType<CellRendererProps> | undefined {
  return CELL_RENDERERS[typeName]
}
