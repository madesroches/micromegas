import type { Table } from 'apache-arrow'
import type { CellRendererProps } from './cell-registry'
import { getCellTypeMetadata } from './cell-registry'
import type { CellConfig, CellState, CellStatus, VariableCellConfig, VariableValue } from './notebook-types'

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
  /** Upstream cell result tables (for $cell[N].col macro substitution) */
  cellResults: Record<string, Table>
  /** Selected rows from upstream cells (for $cell.selected.col macro substitution) */
  cellSelections: Record<string, Record<string, unknown>>
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

/**
 * Safely compute the total byte size of an Arrow Table's batches.
 * Guards against undefined buffers in Data objects that can occur when
 * the WASM Arrow IPC writer produces 0-row results deserialized by the
 * JS Arrow library.
 */
export function safeTableByteLength(table: Table): number {
  if (table.numRows === 0) return 0
  return table.batches.reduce((sum, batch) => {
    try {
      return sum + batch.data.byteLength
    } catch {
      return sum
    }
  }, 0)
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
      (sum, t) => sum + safeTableByteLength(t),
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
 * Compute the aggregate CellStatus for an HG group based on children states.
 * Priority: error > loading > blocked > success > idle
 */
export function computeHgStatus(
  children: CellConfig[],
  cellStates: Record<string, CellState>,
): CellStatus {
  let hasLoading = false
  let hasError = false
  let hasBlocked = false
  let hasSuccess = false

  for (const child of children) {
    const state = cellStates[child.name]
    if (!state) continue
    switch (state.status) {
      case 'error':
        hasError = true
        break
      case 'loading':
        hasLoading = true
        break
      case 'blocked':
        hasBlocked = true
        break
      case 'success':
        hasSuccess = true
        break
    }
  }

  if (hasError) return 'error'
  if (hasLoading) return 'loading'
  if (hasBlocked) return 'blocked'
  if (hasSuccess) return 'success'
  return 'idle'
}

/**
 * Aggregate status for an HG group: total rows, total bytes, sum of elapsed
 * times across all children that have data.  During loading, shows live
 * fetch progress aggregated across all loading children.
 */
export function buildHgStatusText(
  children: CellConfig[],
  cellStates: Record<string, CellState>,
): string | undefined {
  // During loading, aggregate fetch progress from loading children
  let loadingRows = 0
  let loadingBytes = 0
  let hasLoadingProgress = false

  for (const child of children) {
    const state = cellStates[child.name]
    if (state?.status === 'loading' && state.fetchProgress) {
      hasLoadingProgress = true
      loadingRows += state.fetchProgress.rows
      loadingBytes += state.fetchProgress.bytes
    }
  }

  if (hasLoadingProgress) {
    return `${loadingRows.toLocaleString()} rows (${formatBytes(loadingBytes)})`
  }

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
      (sum, t) => sum + safeTableByteLength(t),
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
    cellResults: context.cellResults,
    cellSelections: context.cellSelections,
    // Metadata rendererProps spread last (highest precedence) — preserves
    // current behavior where getRendererProps can override base fields like
    // data, status, options.
    ...rendererProps,
  }
}
