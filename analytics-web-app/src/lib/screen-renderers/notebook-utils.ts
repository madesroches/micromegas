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

import type { Table, DataType } from 'apache-arrow'
import type { CellConfig, CellType, HorizontalGroupCellConfig, VariableValue } from './notebook-types'
import { isTimeType, timestampToDate } from '@/lib/arrow-utils'
import { getVariableString } from './notebook-types'

import type { ScreenConfig } from '@/lib/screens-api'
import { RESERVED_URL_PARAMS } from '@/lib/url-cleanup-utils'

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
  return null // Valid
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
}

// ============================================================================
// Pure Functions
// ============================================================================

/**
 * Sanitizes a cell name to be a valid identifier for variable substitution.
 * - Converts spaces to underscores
 * - Removes non-ASCII characters
 * - Ensures name starts with a letter or underscore
 */
export function sanitizeCellName(name: string): string {
  let sanitized = name
    // Replace spaces with underscores
    .replace(/\s+/g, '_')
    // Remove non-ASCII characters
    // eslint-disable-next-line no-control-regex
    .replace(/[^\x00-\x7F]/g, '')
    // Remove characters that aren't alphanumeric or underscore
    .replace(/[^a-zA-Z0-9_]/g, '')

  // Ensure name starts with a letter or underscore
  if (sanitized && /^[0-9]/.test(sanitized)) {
    sanitized = '_' + sanitized
  }

  return sanitized
}

/**
 * Validates a cell name and returns an error message if invalid.
 * Returns null if the name is valid.
 *
 * @param name - The name to validate
 * @param existingNames - Set of existing cell names
 * @param currentName - The cell's current name (for uniqueness check)
 * @param isVariable - Whether this is a variable cell (checks reserved names)
 */
export function validateCellName(
  name: string,
  existingNames: Set<string>,
  currentName?: string,
  isVariable?: boolean
): string | null {
  if (!name || name.trim() === '') {
    return 'Cell name cannot be empty'
  }

  // Check for non-ASCII characters
  // eslint-disable-next-line no-control-regex
  if (/[^\x00-\x7F]/.test(name)) {
    return 'Cell name can only contain ASCII characters'
  }

  // Check for invalid identifier characters
  if (/[^a-zA-Z0-9_ ]/.test(name)) {
    return 'Cell name can only contain letters, numbers, underscores, and spaces'
  }

  // Check uniqueness (excluding current cell's name)
  const normalizedName = sanitizeCellName(name)
  const normalizedExisting = new Set(
    [...existingNames]
      .filter((n) => n !== currentName)
      .map((n) => sanitizeCellName(n))
  )
  if (normalizedExisting.has(normalizedName)) {
    return 'A cell with this name already exists'
  }

  // Check for reserved names (only for variable cells)
  if (isVariable && RESERVED_URL_PARAMS.has(normalizedName)) {
    return `"${normalizedName}" is reserved and cannot be used for variables`
  }

  return null
}

/**
 * Mutates params: removes variable URL params that match saved cell defaults.
 * Only touches non-reserved params.
 */
export function cleanupVariableParams(
  params: URLSearchParams,
  savedConfig: ScreenConfig,
): void {
  type SavedCell = { type: string; name: string; defaultValue?: string; children?: SavedCell[] }
  const savedCells = (savedConfig as { cells?: SavedCell[] }).cells
  if (!savedCells) return

  // Flatten saved cells to include hg children
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
 * Format an Arrow value as a string, converting timestamps to RFC3339.
 */
function formatArrowValue(value: unknown, dataType?: DataType): string {
  if (dataType && isTimeType(dataType)) {
    const date = timestampToDate(value, dataType)
    if (date) return date.toISOString()
  }
  return String(value)
}

/**
 * Escapes single quotes in a value for SQL safety.
 */
function escapeSqlValue(value: string): string {
  return value.replace(/'/g, "''")
}

// ==========================================================================
// Macro regex patterns
// ==========================================================================
// Each function returns a fresh RegExp with the 'g' flag so callers get
// independent lastIndex state.  Defined once to avoid drift between
// substituteMacros, validateMacros, and findUnresolvedSelectionMacro.

/** $cell[N].column — cell result row reference */
const cellRefRegex = () => /\$([a-zA-Z_][a-zA-Z0-9_]*)\[(\d+)\]\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g

/** $cell.selected.column — selected row reference (captures cell + column) */
const selectedRefRegex = () => /\$([a-zA-Z_][a-zA-Z0-9_]*)\.selected\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g

/** $variable.column — dotted variable reference */
const dottedVarRegex = () => /\$([a-zA-Z_][a-zA-Z0-9_]*)\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g

/** $variable (not followed by . or [) — simple variable reference */
const simpleVarRegex = () => /\$([a-zA-Z_][a-zA-Z0-9_]*)\b(?![.[])/g

/**
 * Substitutes macros in SQL with variable values and time range.
 * - $from and $to are replaced with timestamp values (user controls quoting)
 * - $variable.column syntax accesses specific columns in multi-column variables
 * - $variable syntax accesses the first column value (or the string value for simple variables)
 * - User variables are replaced without quotes (SQL author controls quoting)
 * - Single quotes in values are escaped for SQL safety
 */
export function substituteMacros(
  sql: string,
  variables: Record<string, VariableValue>,
  timeRange: { begin: string; end: string },
  cellResults?: Record<string, Table>,
  cellSelections?: Record<string, Record<string, unknown>>,
): string {
  let result = sql

  // 1. Substitute $from and $to (user controls quoting, like other variables)
  result = result.replace(/\$from\b/g, escapeSqlValue(timeRange.begin))
  result = result.replace(/\$to\b/g, escapeSqlValue(timeRange.end))

  // 2. Cell result row references: $cell[N].column
  //    Must process before dotted/simple variables to avoid partial matches
  if (cellResults) {
    result = result.replace(cellRefRegex(), (match, cellName, rowIdxStr, colName) => {
      const table = cellResults[cellName]
      if (!table) return match // leave unresolved
      const rowIdx = parseInt(rowIdxStr, 10)
      if (rowIdx >= table.numRows) return match
      const row = table.get(rowIdx)
      if (!row || row[colName] === undefined || row[colName] === null) return match
      const field = table.schema.fields.find((f) => f.name === colName)
      return escapeSqlValue(formatArrowValue(row[colName], field?.type))
    })
  }

  // 2b. Selected row references: $cell.selected.column
  //     Must process before dotted variable pass to avoid partial matches.
  //     When no selection exists, resolves to empty string.
  if (cellSelections) {
    result = result.replace(selectedRefRegex(), (match, cellName, colName) => {
      const selection = cellSelections[cellName]
      if (!selection) return ''
      const value = selection[colName]
      if (value === undefined || value === null) return ''
      const table = cellResults?.[cellName]
      const field = table?.schema.fields.find((f) => f.name === colName)
      return escapeSqlValue(formatArrowValue(value, field?.type))
    })
  }

  // 3. Handle dotted variable references first: $variable.column
  //    Must process before simple variables to avoid partial matches
  result = result.replace(dottedVarRegex(), (match, varName, colName) => {
    const value = variables[varName]
    if (value === undefined) return match // Leave unresolved

    if (typeof value === 'string') {
      // Simple variable doesn't have columns - leave unresolved
      return match
    }

    const colValue = value[colName]
    if (colValue === undefined) {
      return match
    }

    return escapeSqlValue(colValue)
  })

  // 3. Handle simple variable references: $variable
  //    Sort by name length descending to avoid partial matches ($metric vs $metric_name)
  //    Use negative lookahead (?!\.) to avoid matching $variable in $variable.column
  const sortedVars = Object.entries(variables).sort((a, b) => b[0].length - a[0].length)
  for (const [name, value] of sortedVars) {
    const regex = new RegExp(`\\$${name}\\b(?![.\\[])`, 'g')

    if (typeof value === 'string') {
      result = result.replace(regex, escapeSqlValue(value))
    } else {
      // Multi-column variable referenced without column - use first column value
      const firstValue = getVariableString(value)
      result = result.replace(regex, escapeSqlValue(firstValue))
    }
  }

  return result
}

/**
 * Result of macro validation.
 */
export interface MacroValidationResult {
  valid: boolean
  errors: string[]
}

/**
 * Validates macro references in text against available variables.
 * Returns errors for unknown variables or invalid column references.
 */
export function validateMacros(
  text: string,
  variables: Record<string, VariableValue>,
  cellResults?: Record<string, Table>,
  cellSelections?: Record<string, Record<string, unknown>>,
): MacroValidationResult {
  const errors: string[] = []

  // Check cell result references: $cell[N].column
  const cellRefPattern = cellRefRegex()
  let match
  while ((match = cellRefPattern.exec(text)) !== null) {
    const [, cellName, rowIdxStr, colName] = match
    if (!cellResults) continue
    const table = cellResults[cellName]
    if (!table) {
      errors.push(`Unknown cell: ${cellName}`)
    } else {
      const rowIdx = parseInt(rowIdxStr, 10)
      if (rowIdx >= table.numRows) {
        errors.push(`Row index ${rowIdx} out of bounds for cell '${cellName}' (${table.numRows} rows)`)
      } else {
        const columns = table.schema.fields.map(f => f.name)
        if (!columns.includes(colName)) {
          errors.push(`Column '${colName}' not found in cell '${cellName}'. Available: ${columns.join(', ')}`)
        }
      }
    }
  }

  // Check selected row references: $cell.selected.column
  const selectedRefPattern = selectedRefRegex()
  while ((match = selectedRefPattern.exec(text)) !== null) {
    const [, cellName, colName] = match
    if (!cellSelections) continue
    if (!(cellName in cellSelections)) {
      errors.push(`Unknown cell: ${cellName}`)
    } else {
      const selection = cellSelections[cellName]
      if (selection && selection[colName] === undefined) {
        const columns = Object.keys(selection)
        errors.push(`Column '${colName}' not found in cell '${cellName}'. Available: ${columns.join(', ')}`)
      }
    }
  }

  // Collect selected ref cell names so dotted validation skips them
  const selectedRefCellNames = new Set<string>()
  const selectedRefScan = selectedRefRegex()
  while ((match = selectedRefScan.exec(text)) !== null) {
    selectedRefCellNames.add(match[1])
  }

  // Check dotted references: $variable.column (skip $cell.selected.column which was handled above)
  const dottedPattern = dottedVarRegex()
  while ((match = dottedPattern.exec(text)) !== null) {
    const [, varName, colName] = match
    // Skip $cell.selected (part of $cell.selected.column pattern)
    if (colName === 'selected' && selectedRefCellNames.has(varName)) continue
    const value = variables[varName]

    if (value === undefined) {
      errors.push(`Unknown variable: ${varName}`)
    } else if (typeof value === 'string') {
      errors.push(
        `Variable '${varName}' is not a multi-column variable, cannot access '${colName}'`
      )
    } else if (value[colName] === undefined) {
      errors.push(
        `Column '${colName}' not found in variable '${varName}'. Available: ${Object.keys(value).join(', ')}`
      )
    }
  }

  // Check simple variable references: $variable (not followed by a dot or bracket)
  const simplePattern = simpleVarRegex()
  while ((match = simplePattern.exec(text)) !== null) {
    const [, varName] = match
    // Skip built-in variables
    if (varName === 'from' || varName === 'to' || varName === 'order_by') {
      continue
    }
    if (variables[varName] === undefined) {
      errors.push(`Unknown variable: ${varName}`)
    }
  }

  return { valid: errors.length === 0, errors }
}

/**
 * Checks if a SQL string contains unresolved $cell.selected.column macros.
 * Returns the cell name if found, null otherwise.
 */
export function findUnresolvedSelectionMacro(
  sql: string,
  cellSelections?: Record<string, Record<string, unknown>>,
): string | null {
  const pattern = selectedRefRegex()
  let match
  while ((match = pattern.exec(sql)) !== null) {
    const cellName = match[1]
    if (!cellSelections || !cellSelections[cellName]) {
      return cellName
    }
  }
  return null
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
