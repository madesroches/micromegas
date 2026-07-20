/**
 * Notebook cell utilities: traversal, name sanitization/validation, defaults,
 * data-source resolution.
 *
 * The macro engine is split across sibling modules:
 *   - SQL macro substitution + validation → `./macro-substitution`
 *   - Markdown template evaluator (function calls + raw-value macros) → `./template-evaluator`
 *
 * Both are re-exported here for backward compatibility with existing call sites.
 */

// Re-export types from notebook-types for backwards compatibility
export type {
  CellType,
  CellStatus,
  CellConfigBase,
  QueryCellConfig,
  MarkdownCellConfig,
  VariableCellConfig,
  HorizontalGroupCellConfig,
  CellConfig,
  CellState,
  NotebookConfig,
  VariableValue,
} from './notebook-types'

export {
  getVariableString,
  isMultiColumnValue,
  serializeVariableValue,
  deserializeVariableValue,
  variableValuesEqual,
} from './notebook-types'

// Re-export the macro substitution + validation surface (legacy SQL path).
export {
  formatArrowValue,
  substituteMacros,
  substituteMacrosRaw,
  validateMacros,
  findUnresolvedSelectionMacro,
} from './macro-substitution'
export type { MacroValidationResult } from './macro-substitution'

// Re-export the template evaluator surface (Markdown / function-call path).
export { evaluateTemplate } from './template-evaluator'
export type { EvaluateTemplateCtx, EvaluateTemplateResult } from './template-evaluator'

import type { Table } from 'apache-arrow'
import type { CellConfig, CellType, HorizontalGroupCellConfig, QueryBackedCellConfig, VariableValue } from './notebook-types'
import { substituteMacrosRaw } from './macro-substitution'

import type { ScreenConfig } from '@/lib/screens-api'
import { RESERVED_URL_PARAMS } from '@/lib/url-cleanup-utils'
import { parseRelativeTime } from '@/lib/time-range'

/**
 * Checks if a variable name conflicts with reserved URL parameter names.
 * Uses the sanitized form of the name for comparison.
 */
export function isReservedVariableName(name: string): boolean {
  const sanitized = sanitizeCellName(name)
  return RESERVED_URL_PARAMS.has(sanitized)
}

/**
 * Validates a variable name for URL sync compatibility.
 * Returns an error message if the name would conflict with reserved params.
 */
export function validateVariableName(name: string): string | null {
  const sanitized = sanitizeCellName(name)
  if (RESERVED_URL_PARAMS.has(sanitized)) {
    return `"${sanitized}" is a reserved name and cannot be used for variables (conflicts with URL parameters)`
  }
  return null
}

/**
 * Visits every leaf cell in a cell list, expanding hg group children in place.
 * The hg cell itself is skipped; its children are visited left to right.
 */
export function forEachCell(cells: CellConfig[], fn: (cell: CellConfig) => void): void {
  for (const cell of cells) {
    if (cell.type === 'hg') {
      for (const child of (cell as HorizontalGroupCellConfig).children) {
        fn(child)
      }
    } else {
      fn(cell)
    }
  }
}

/**
 * Flattens a cell list for execution by expanding hg children into the top-level list.
 * The hg cell itself is omitted; its children appear in its place (left to right).
 */
export function flattenCellsForExecution(cells: CellConfig[]): CellConfig[] {
  const result: CellConfig[] = []
  forEachCell(cells, (cell) => result.push(cell))
  return result
}

/**
 * Collects all cell names including children inside hg groups.
 */
export function collectAllCellNames(cells: CellConfig[]): Set<string> {
  const names = new Set<string>()
  for (const cell of cells) {
    names.add(cell.name)
    if (cell.type === 'hg') {
      for (const child of (cell as HorizontalGroupCellConfig).children) {
        names.add(child.name)
      }
    }
  }
  return names
}

/**
 * Returns true if a cell type should display a data source selector.
 * Markdown cells have no queries, variable cells handle their own selector,
 * referencetable cells don't query, and chart cells manage data source per-query.
 */
export function shouldShowDataSource(type: CellType): boolean {
  return type !== 'markdown' && type !== 'variable' && type !== 'referencetable' && type !== 'chart'
}

// Default SQL queries per cell type
export const DEFAULT_SQL: Record<string, string> = {
  table: `SELECT process_id, exe, start_time, last_update_time, username, computer
FROM processes
ORDER BY last_update_time DESC
LIMIT 100`,
  chart: `SELECT time, value
FROM measures
WHERE name = 'cpu_usage'
ORDER BY time
LIMIT 100`,
  log: `SELECT time, level, target, msg
FROM log_entries
ORDER BY time DESC
LIMIT 100`,
  variable: `SELECT DISTINCT name FROM measures`,
  propertytimeline: `WITH changes AS (
  SELECT
    time,
    jsonb_format_json(properties) as properties,
    LAG(jsonb_format_json(properties)) OVER (ORDER BY time) as prev_properties
  FROM view_instance('measures', '$process_id')
  WHERE name = 'cpu_usage'
)
SELECT time, properties
FROM changes
WHERE properties IS DISTINCT FROM prev_properties
ORDER BY time`,
  swimlane: `SELECT
  arrow_cast(stream_id, 'Utf8') as id,
  concat(
    arrow_cast(property_get("streams.properties", 'thread-name'), 'Utf8'),
    '-',
    arrow_cast(property_get("streams.properties", 'thread-id'), 'Utf8')
  ) as name,
  begin_time as begin,
  end_time as end
FROM blocks
WHERE process_id = '$process_id'
  AND array_has("streams.tags", 'cpu')
ORDER BY name, begin`,
  transposed: `SELECT 1 as value`,
  flamegraph: `SELECT id, parent, name, begin, "end", depth, thread_name as lane
FROM process_spans('$process_id', 'both')
ORDER BY lane, begin`,
  map: `SELECT NOW() as time, 0.0 as x, 0.0 as y, 0.0 as z`,
}

/**
 * Default Markdown template for the Map cell's event detail panel.
 * Renders the canonical x/y/z columns; authors extend this in the editor
 * to surface their own query columns (e.g. process_id, event_type).
 */
export const DEFAULT_MAP_DETAIL_TEMPLATE = `### Event Details

---

**Location:** ($x, $y, $z)
`

// ============================================================================
// Cell name sanitization / validation
// ============================================================================

/**
 * Sanitizes a cell name to be a valid identifier for variable substitution.
 * - Converts spaces to underscores
 * - Removes non-ASCII characters
 * - Ensures name starts with a letter or underscore
 */
export function sanitizeCellName(name: string): string {
  let sanitized = name
    .replace(/\s+/g, '_')
    // eslint-disable-next-line no-control-regex
    .replace(/[^\x00-\x7F]/g, '')
    .replace(/[^a-zA-Z0-9_]/g, '')

  if (sanitized && /^[0-9]/.test(sanitized)) {
    sanitized = '_' + sanitized
  }

  return sanitized
}

/**
 * Validates a cell name and returns an error message if invalid.
 * Returns null if the name is valid.
 */
export function validateCellName(
  name: string,
  existingNames: Set<string>,
  currentName?: string,
  isVariable?: boolean,
): string | null {
  if (!name || name.trim() === '') {
    return 'Cell name cannot be empty'
  }

  // eslint-disable-next-line no-control-regex
  if (/[^\x00-\x7F]/.test(name)) {
    return 'Cell name can only contain ASCII characters'
  }

  if (/[^a-zA-Z0-9_ ]/.test(name)) {
    return 'Cell name can only contain letters, numbers, underscores, and spaces'
  }

  const normalizedName = sanitizeCellName(name)
  const normalizedExisting = new Set(
    [...existingNames].filter((n) => n !== currentName).map((n) => sanitizeCellName(n)),
  )
  if (normalizedExisting.has(normalizedName)) {
    return 'A cell with this name already exists'
  }

  if (isVariable && RESERVED_URL_PARAMS.has(normalizedName)) {
    return `"${normalizedName}" is reserved and cannot be used for variables`
  }

  return null
}

/**
 * Mutates params: removes variable URL params that match saved cell defaults.
 * Only touches non-reserved params.
 */
export function cleanupVariableParams(params: URLSearchParams, savedConfig: ScreenConfig): void {
  type SavedCell = { type: string; name: string; defaultValue?: string; children?: SavedCell[] }
  const savedCells = (savedConfig as { cells?: SavedCell[] }).cells
  if (!savedCells) return

  const allSavedCells: SavedCell[] = []
  for (const cell of savedCells) {
    allSavedCells.push(cell)
    if (cell.type === 'hg' && cell.children) {
      allSavedCells.push(...cell.children)
    }
  }

  const keysToDelete: string[] = []
  params.forEach((_value, key) => {
    if (RESERVED_URL_PARAMS.has(key)) return
    const savedCell = allSavedCells.find((c) => c.type === 'variable' && c.name === key)
    if (savedCell && savedCell.defaultValue === params.get(key)) {
      keysToDelete.push(key)
    }
  })

  for (const key of keysToDelete) {
    params.delete(key)
  }
}

/**
 * Resolve a cell's data source, substituting $varname references
 * with the corresponding variable value. Falls back to the notebook-level
 * data source when the variable is missing or empty.
 */
export function resolveCellDataSource(
  cell: CellConfig,
  variables: Record<string, VariableValue>,
  notebookDataSource: string | undefined,
): string | undefined {
  let ds = ('dataSource' in cell ? cell.dataSource : undefined) || notebookDataSource
  if (ds?.startsWith('$')) {
    const varValue = variables[ds.slice(1)]
    ds = (typeof varValue === 'string' && varValue) ? varValue : notebookDataSource
  }
  return ds
}

// ============================================================================
// Per-cell query time range
// ============================================================================

interface MacroCtx {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
}

/**
 * Resolve a cell's optional timeRange override to a runtime { begin, end }.
 * Each empty/unset bound falls back to the global range. Throws on an
 * unparseable bound (caller decides whether to error the cell or fall back).
 */
export function resolveQueryTimeRange(
  config: CellConfig,
  ctx: MacroCtx,
): { begin: string; end: string } {
  const raw = 'timeRange' in config ? (config as QueryBackedCellConfig).timeRange : undefined
  const fromStr = raw?.from?.trim() || ''
  const toStr = raw?.to?.trim() || ''
  if (!fromStr && !toStr) return ctx.timeRange

  const resolveBound = (s: string, fallback: string): string => {
    if (!s) return fallback
    const substituted = substituteMacrosRaw(s, ctx.variables, ctx.timeRange, ctx.cellResults, ctx.cellSelections)
    return parseRelativeTime(substituted).toISOString()
  }

  return {
    begin: resolveBound(fromStr, ctx.timeRange.begin),
    end: resolveBound(toStr, ctx.timeRange.end),
  }
}

/**
 * Returns true if a cell type should show the per-cell query time range field.
 * Distinct from `shouldShowDataSource`, which excludes `chart` for per-query
 * data-source reasons that don't apply to the cell-level time window.
 */
export function shouldShowTimeRange(cell: CellConfig): boolean {
  switch (cell.type) {
    case 'markdown':
    case 'referencetable':
    case 'hg':
      return false
    case 'variable':
      return cell.variableType === 'combobox' || cell.variableType === 'expression'
    default:
      return true // table, chart, log, propertytimeline, swimlane, transposed, flamegraph, map, perfettoexport, image
  }
}
