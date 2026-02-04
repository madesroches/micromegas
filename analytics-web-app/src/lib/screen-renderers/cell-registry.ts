import { ComponentType } from 'react'
import { Table } from 'apache-arrow'
import type { CellConfig, CellState, CellType, CellStatus, VariableValue } from './notebook-types'

// Re-export types from notebook-types for backwards compatibility
export type { CellType, CellStatus, CellConfig, CellState, VariableValue }

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
  variables: Record<string, VariableValue>
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
  value?: VariableValue
  /** For variable cells: update value */
  onValueChange?: (value: VariableValue) => void
  /** For variable cells: variable type */
  variableType?: 'combobox' | 'text' | 'number'
  /** For variable cells (combobox): available options from query */
  variableOptions?: { label: string; value: VariableValue }[]
  /** Callback for drag-to-zoom time selection (chart and property timeline cells) */
  onTimeRangeSelect?: (from: Date, to: Date) => void
}

/**
 * Context provided to cell execution.
 */
export interface CellExecutionContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>
}

/**
 * Props for cell-specific editor components.
 * Includes variables/timeRange so editors can show available variables panel if needed.
 */
export interface CellEditorProps {
  config: CellConfig
  onChange: (config: CellConfig) => void
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  /** Available column names from query results (for table/chart cells) */
  availableColumns?: string[]
}

/**
 * Metadata describing a cell type's behavior and components.
 * Each cell type exports a single metadata object that fully describes it.
 */
export interface CellTypeMetadata {
  /** Renderer component (displays cell output) */
  readonly renderer: ComponentType<CellRendererProps>

  /** Editor component (type-specific config fields only) */
  readonly EditorComponent: ComponentType<CellEditorProps>

  /** Display name (e.g., "Table", "Chart") */
  readonly label: string

  /** Icon for the cell type (single character, e.g., "T", "C") */
  readonly icon: string

  /** Description for add cell modal */
  readonly description: string

  /** Whether to show type badge in cell header (false for markdown) */
  readonly showTypeBadge: boolean

  /** Default height for new cells */
  readonly defaultHeight: number

  /** Whether this cell can block downstream execution (false for markdown) */
  readonly canBlockDownstream: boolean

  /** Creates default config for a new cell of this type */
  readonly createDefaultConfig: () => Omit<CellConfig, 'name' | 'layout'>

  /**
   * Executes the cell and returns state updates.
   * Returns null if nothing to execute (e.g., text/number variables).
   * Absence of this method means the cell doesn't execute (e.g., markdown).
   */
  readonly execute?: (
    config: CellConfig,
    context: CellExecutionContext
  ) => Promise<Partial<CellState> | null>

  /**
   * Post-execution hook (e.g., auto-select/validate value for variables).
   * Called after successful execution to validate current value or set default.
   * @param currentValue - The current variable value (if any) from URL config
   */
  readonly onExecutionComplete?: (
    config: CellConfig,
    state: CellState,
    context: {
      setVariableValue: (name: string, value: VariableValue) => void
      currentValue: VariableValue | undefined
    }
  ) => void

  /** Extracts props for the renderer from config and state */
  readonly getRendererProps: (config: CellConfig, state: CellState) => Partial<CellRendererProps>
}

// Import metadata from each cell file
import { tableMetadata } from './cells/TableCell'
import { chartMetadata } from './cells/ChartCell'
import { logMetadata } from './cells/LogCell'
import { markdownMetadata } from './cells/MarkdownCell'
import { variableMetadata } from './cells/VariableCell'
import { propertyTimelineMetadata } from './cells/PropertyTimelineCell'
import { swimlaneMetadata } from './cells/SwimlaneCell'
import { perfettoExportMetadata } from './cells/PerfettoExportCell'

/**
 * Registry of all cell type metadata.
 * This is a static map built from explicit imports - no runtime registration needed.
 */
export const CELL_TYPE_METADATA: Record<CellType, CellTypeMetadata> = {
  table: tableMetadata,
  chart: chartMetadata,
  log: logMetadata,
  markdown: markdownMetadata,
  variable: variableMetadata,
  propertytimeline: propertyTimelineMetadata,
  swimlane: swimlaneMetadata,
  perfettoexport: perfettoExportMetadata,
}

/**
 * Get metadata for a cell type.
 */
export function getCellTypeMetadata(type: CellType): CellTypeMetadata {
  return CELL_TYPE_METADATA[type]
}

/**
 * Get the renderer component for a cell type.
 */
export function getCellRenderer(type: CellType): ComponentType<CellRendererProps> {
  return CELL_TYPE_METADATA[type].renderer
}

/**
 * Get the editor component for a cell type.
 */
export function getCellEditor(type: CellType): ComponentType<CellEditorProps> {
  return CELL_TYPE_METADATA[type].EditorComponent
}

/**
 * Cell type options derived from metadata (for add cell modal).
 */
export const CELL_TYPE_OPTIONS = (Object.entries(CELL_TYPE_METADATA) as [CellType, CellTypeMetadata][]).map(
  ([type, meta]) => ({
    type,
    name: meta.label,
    description: meta.description,
    icon: meta.icon,
  })
)

/**
 * Creates a default cell configuration for the given type.
 * Generates a unique name if the base name already exists.
 */
export function createDefaultCell(type: CellType, existingNames: Set<string>): CellConfig {
  const meta = CELL_TYPE_METADATA[type]

  // Generate unique name (use underscore separator for valid identifiers)
  let name = meta.label
  let counter = 1
  while (existingNames.has(name)) {
    counter++
    name = `${meta.label}_${counter}`
  }

  return {
    name,
    layout: { height: meta.defaultHeight },
    ...meta.createDefaultConfig(),
  } as CellConfig
}
