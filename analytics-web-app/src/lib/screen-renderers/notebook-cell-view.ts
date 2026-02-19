import type { CellRendererProps } from './cell-registry'
import { getCellTypeMetadata } from './cell-registry'
import type { CellConfig, CellState, VariableCellConfig, VariableValue } from './notebook-types'

// =============================================================================
// Interfaces
// =============================================================================

export interface CellViewContext {
  /** Scoped variables visible to this cell (from cells above — used for query substitution) */
  availableVariables: Record<string, VariableValue>
  /** All variable values (used to look up this cell's own value for variable cells) */
  allVariableValues: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  isEditing: boolean
  dataSource?: string
}

export interface CellViewCallbacks {
  onRun: () => void
  onSqlChange: (sql: string) => void
  onOptionsChange: (options: Record<string, unknown>) => void
  onContentChange?: (content: string) => void
  onValueChange?: (value: VariableValue) => void
  onTimeRangeSelect?: (from: Date, to: Date) => void
}

// =============================================================================
// Formatting helpers
// =============================================================================

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

export function formatElapsedMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`
  return `${(ms / 1000).toFixed(2)}s`
}

// =============================================================================
// Status text builders
// =============================================================================

export function buildStatusText(
  cell: CellConfig,
  state: CellState,
): string | undefined {
  // Non-combobox variable cells never show status text
  if (
    cell.type === 'variable' &&
    (cell as VariableCellConfig).variableType !== 'combobox'
  ) {
    return undefined
  }

  // During loading, show fetch progress if available
  if (state.status === 'loading' && state.fetchProgress) {
    return `${state.fetchProgress.rows.toLocaleString()} rows (${formatBytes(state.fetchProgress.bytes)})`
  }

  // Show final data stats
  if (state.data && state.data.length > 0) {
    const totalRows = state.data.reduce((sum, t) => sum + t.numRows, 0)
    const totalBytes = state.data.reduce(
      (sum, t) =>
        sum + t.batches.reduce((s: number, b) => s + b.data.byteLength, 0),
      0,
    )
    const rowText = `${totalRows.toLocaleString()} rows (${formatBytes(totalBytes)})`
    return state.elapsedMs != null
      ? `${rowText} in ${formatElapsedMs(state.elapsedMs)}`
      : rowText
  }

  return undefined
}

/**
 * Aggregate status for an HG group: total rows, total bytes, sum of elapsed
 * times across all children that have data.
 */
export function buildHgStatusText(
  children: CellConfig[],
  cellStates: Record<string, CellState>,
): string | undefined {
  let totalRows = 0
  let totalBytes = 0
  let totalElapsed = 0
  let hasData = false
  let allHaveElapsed = true

  for (const child of children) {
    const state = cellStates[child.name]
    if (!state?.data || state.data.length === 0) continue

    hasData = true
    totalRows += state.data.reduce((sum, t) => sum + t.numRows, 0)
    totalBytes += state.data.reduce(
      (sum, t) =>
        sum + t.batches.reduce((s: number, b) => s + b.data.byteLength, 0),
      0,
    )
    if (state.elapsedMs != null) {
      totalElapsed += state.elapsedMs
    } else {
      allHaveElapsed = false
    }
  }

  if (!hasData) return undefined

  const rowText = `${totalRows.toLocaleString()} rows (${formatBytes(totalBytes)})`
  return hasData && allHaveElapsed && totalElapsed > 0
    ? `${rowText} in ${formatElapsedMs(totalElapsed)}`
    : rowText
}

// =============================================================================
// Prop assembly
// =============================================================================

export function buildCellRendererProps(
  cell: CellConfig,
  state: CellState,
  context: CellViewContext,
  callbacks: CellViewCallbacks,
): CellRendererProps {
  const meta = getCellTypeMetadata(cell.type)
  const rendererProps = meta.getRendererProps(cell, state)

  return {
    name: cell.name,
    data: state.data,
    status: state.status,
    error: state.error,
    timeRange: context.timeRange,
    variables: context.availableVariables,
    isEditing: context.isEditing,
    onRun: callbacks.onRun,
    onSqlChange: callbacks.onSqlChange,
    onOptionsChange: callbacks.onOptionsChange,
    onContentChange: callbacks.onContentChange,
    onTimeRangeSelect: callbacks.onTimeRangeSelect,
    value:
      cell.type === 'variable'
        ? context.allVariableValues[cell.name]
        : undefined,
    onValueChange:
      cell.type === 'variable' ? callbacks.onValueChange : undefined,
    dataSource: context.dataSource,
    // Metadata rendererProps spread last (highest precedence) — preserves
    // current behavior where getRendererProps can override base fields like
    // data, status, options.
    ...rendererProps,
  }
}
