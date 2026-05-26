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
import { TEMPLATE_FUNCTIONS } from '@/lib/template-functions'

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
export function formatArrowValue(value: unknown, dataType?: DataType): string {
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
  cellResults: Record<string, Table>,
  cellSelections: Record<string, Record<string, unknown>>,
): string {
  return substituteMacrosImpl(sql, variables, timeRange, cellResults, cellSelections, escapeSqlValue)
}

/**
 * Substitutes macros without SQL escaping — returns the raw resolved value.
 * Used by non-SQL contexts (e.g. parsing a macro-driven scalar into a number
 * or a hex color). Same regex/lookup rules as `substituteMacros`.
 */
export function substituteMacrosRaw(
  input: string,
  variables: Record<string, VariableValue>,
  timeRange: { begin: string; end: string },
  cellResults: Record<string, Table>,
  cellSelections: Record<string, Record<string, unknown>>,
): string {
  return substituteMacrosImpl(input, variables, timeRange, cellResults, cellSelections, (s) => s)
}

function substituteMacrosImpl(
  input: string,
  variables: Record<string, VariableValue>,
  timeRange: { begin: string; end: string },
  cellResults: Record<string, Table>,
  cellSelections: Record<string, Record<string, unknown>>,
  escape: (value: string) => string,
): string {
  let result = input

  // 1. Substitute $from and $to (user controls quoting, like other variables)
  result = result.replace(/\$from\b/g, escape(timeRange.begin))
  result = result.replace(/\$to\b/g, escape(timeRange.end))

  // 2. Cell result row references: $cell[N].column
  //    Must process before dotted/simple variables to avoid partial matches
  result = result.replace(cellRefRegex(), (match, cellName, rowIdxStr, colName) => {
    const table = cellResults[cellName]
    if (!table) return match // leave unresolved
    const rowIdx = parseInt(rowIdxStr, 10)
    if (rowIdx >= table.numRows) return match
    const row = table.get(rowIdx)
    if (!row || row[colName] === undefined || row[colName] === null) return match
    const field = table.schema.fields.find((f) => f.name === colName)
    return escape(formatArrowValue(row[colName], field?.type))
  })

  // 2b. Selected row references: $cell.selected.column
  //     Must process before dotted variable pass to avoid partial matches.
  //     When no selection exists, resolves to empty string.
  result = result.replace(selectedRefRegex(), (match, cellName, colName) => {
    const selection = cellSelections[cellName]
    if (!selection) return ''
    const value = selection[colName]
    if (value === undefined || value === null) return ''
    const table = cellResults[cellName]
    const field = table?.schema.fields.find((f) => f.name === colName)
    return escape(formatArrowValue(value, field?.type))
  })

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

    return escape(colValue)
  })

  // 3. Handle simple variable references: $variable
  //    Sort by name length descending to avoid partial matches ($metric vs $metric_name)
  //    Use negative lookahead (?!\.) to avoid matching $variable in $variable.column
  const sortedVars = Object.entries(variables).sort((a, b) => b[0].length - a[0].length)
  for (const [name, value] of sortedVars) {
    const regex = new RegExp(`\\$${name}\\b(?![.\\[])`, 'g')

    if (typeof value === 'string') {
      result = result.replace(regex, escape(value))
    } else {
      // Multi-column variable referenced without column - use first column value
      const firstValue = getVariableString(value)
      result = result.replace(regex, escape(firstValue))
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
  cellResults: Record<string, Table>,
  cellSelections: Record<string, Record<string, unknown>>,
): MacroValidationResult {
  const errors: string[] = []

  // Check cell result references: $cell[N].column
  const cellRefPattern = cellRefRegex()
  let match
  while ((match = cellRefPattern.exec(text)) !== null) {
    const [, cellName, rowIdxStr, colName] = match
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
  cellSelections: Record<string, Record<string, unknown>>,
): string | null {
  const pattern = selectedRefRegex()
  let match
  while ((match = pattern.exec(sql)) !== null) {
    const cellName = match[1]
    if (!cellSelections[cellName]) {
      return cellName
    }
  }
  return null
}

// ==========================================================================
// Template evaluator with function calls (`evaluateTemplate`)
// ==========================================================================
//
// `evaluateTemplate` walks a template string once, left-to-right. At each
// position it tries (in order):
//   1. an identifier-in-registry followed by '(' → function call
//   2. a '$' → macro shape
//   3. literal char copy
//
// Unresolved calls and unresolved macros are left as their original source
// text (no half-substituted state). Each problem accumulates a warning so
// callers can surface diagnostics.

export interface EvaluateTemplateCtx {
  variables: Record<string, VariableValue>
  /** When omitted, `$from`/`$to` are treated as unresolved. */
  timeRange?: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
  /** Row dict (Table override path). When present, `$row.col` and
   *  `$row["col"]` resolve to the raw value. */
  row?: Record<string, unknown>
  /** Column-type map for RFC3339 stringification of timestamps emitted
   *  as naked `$row.col` macros outside function-call args. */
  columnTypes?: Map<string, DataType>
}

export interface EvaluateTemplateResult {
  text: string
  warnings: string[]
}

interface MacroMatch {
  /** Original source text spanning the macro. */
  source: string
  /** End index in the input (exclusive). */
  end: number
  /** Human-readable shape used in warning messages. */
  shape: string
  /** Resolved raw value, or undefined when unresolved. */
  value: unknown
  /** Optional Arrow DataType used by `formatArrowValue` for naked emission. */
  dataType?: DataType
}

const IDENT_RE = /[a-zA-Z_][a-zA-Z0-9_]*/y

function matchIdent(text: string, pos: number): string | null {
  IDENT_RE.lastIndex = pos
  const m = IDENT_RE.exec(text)
  return m ? m[0] : null
}

function skipSpaces(text: string, pos: number): number {
  while (pos < text.length && (text[pos] === ' ' || text[pos] === '\t')) pos++
  return pos
}

function tryParseMacro(text: string, pos: number, ctx: EvaluateTemplateCtx): MacroMatch | null {
  if (text[pos] !== '$') return null
  // 1. $from / $to — only recognized when ctx.timeRange is set, otherwise let
  //    them fall through (no shape match) so the walker copies '$' verbatim
  //    and continues. This preserves the documented "treat as unresolved"
  //    behavior while keeping the walker uniform.
  if (text.startsWith('$from', pos) && !isIdentChar(text[pos + 5] ?? '')) {
    if (!ctx.timeRange) return null
    return { source: '$from', end: pos + 5, shape: '$from', value: ctx.timeRange.begin }
  }
  if (text.startsWith('$to', pos) && !isIdentChar(text[pos + 3] ?? '')) {
    if (!ctx.timeRange) return null
    return { source: '$to', end: pos + 3, shape: '$to', value: ctx.timeRange.end }
  }

  // 2-6. All remaining shapes begin with an identifier after '$'.
  const ident = matchIdent(text, pos + 1)
  if (!ident) return null
  const afterIdent = pos + 1 + ident.length

  // 2. $cell[N].col
  if (text[afterIdent] === '[') {
    const close = text.indexOf(']', afterIdent + 1)
    if (close < 0) return null
    const inside = text.slice(afterIdent + 1, close)
    let nextStart: number
    let colName: string | null = null
    let value: unknown
    let dataType: DataType | undefined
    let shape: string
    let source: string

    if (/^\d+$/.test(inside)) {
      // $cell[N].col
      if (text[close + 1] !== '.') return null
      const colMatch = matchIdent(text, close + 2)
      if (!colMatch) return null
      colName = colMatch
      nextStart = close + 2 + colMatch.length
      source = text.slice(pos, nextStart)
      shape = `$${ident}[${inside}].${colName}`
      const table = ctx.cellResults[ident]
      const rowIdx = parseInt(inside, 10)
      if (table && rowIdx < table.numRows) {
        const row = table.get(rowIdx)
        if (row && row[colName] !== undefined && row[colName] !== null) {
          value = row[colName]
          dataType = table.schema.fields.find((f) => f.name === colName)?.type
        }
      }
      return { source, end: nextStart, shape, value, dataType }
    }

    // $row["col"] / $row['col']
    if (ident === 'row' && ctx.row !== undefined) {
      const m = /^(["'])([^"']+)\1$/.exec(inside)
      if (!m) return null
      colName = m[2]
      nextStart = close + 1
      source = text.slice(pos, nextStart)
      shape = `$row[${inside}]`
      const v = ctx.row[colName]
      if (v !== undefined && v !== null) {
        value = v
        dataType = ctx.columnTypes?.get(colName)
      }
      return { source, end: nextStart, shape, value, dataType }
    }

    return null
  }

  if (text[afterIdent] === '.') {
    const second = matchIdent(text, afterIdent + 1)
    if (!second) return null
    const afterSecond = afterIdent + 1 + second.length

    // 3. $cell.selected.col
    if (second === 'selected' && text[afterSecond] === '.') {
      const colMatch = matchIdent(text, afterSecond + 1)
      if (!colMatch) return null
      const end = afterSecond + 1 + colMatch.length
      const source = text.slice(pos, end)
      const shape = `$${ident}.selected.${colMatch}`
      const selection = ctx.cellSelections[ident]
      let value: unknown
      let dataType: DataType | undefined
      if (selection) {
        const v = selection[colMatch]
        if (v !== undefined && v !== null) {
          value = v
          const table = ctx.cellResults[ident]
          dataType = table?.schema.fields.find((f) => f.name === colMatch)?.type
        }
      }
      return { source, end, shape, value, dataType }
    }

    // 4. $row.col (only when ctx.row present; matched BEFORE $variable.col so a
    //    row reference is never shadowed by a varName='row' lookup).
    if (ident === 'row' && ctx.row !== undefined) {
      const end = afterIdent + 1 + second.length
      const source = text.slice(pos, end)
      const shape = `$row.${second}`
      const v = ctx.row[second]
      if (v !== undefined && v !== null) {
        return {
          source,
          end,
          shape,
          value: v,
          dataType: ctx.columnTypes?.get(second),
        }
      }
      return { source, end, shape, value: undefined }
    }

    // 5. $variable.col
    const end = afterIdent + 1 + second.length
    const source = text.slice(pos, end)
    const shape = `$${ident}.${second}`
    const variable = ctx.variables[ident]
    let value: unknown
    if (variable !== undefined && typeof variable !== 'string') {
      const v = variable[second]
      if (v !== undefined) value = v
    }
    return { source, end, shape, value }
  }

  // 6. $variable
  const end = afterIdent
  const source = text.slice(pos, end)
  const shape = `$${ident}`
  const variable = ctx.variables[ident]
  let value: unknown
  if (variable !== undefined) {
    value = typeof variable === 'string' ? variable : getVariableString(variable)
  }
  return { source, end, shape, value }
}

function isIdentChar(ch: string): boolean {
  return /[a-zA-Z0-9_]/.test(ch)
}

interface CallParseResult {
  /** Resolved args (undefined entries mark unresolved macros). */
  args: unknown[]
  /** Warnings produced during arg parsing (e.g. unresolved macros). */
  argWarnings: string[]
  /** Names of macro shapes that resolved to undefined (for reporting). */
  unresolvedArgShapes: string[]
  /** End index in the input (just past the closing ')'). */
  end: number
  /** True when any structural element failed (bad arg form, missing ')'). */
  aborted: boolean
}

function tryParseCallArgs(
  text: string,
  pos: number,
  ctx: EvaluateTemplateCtx,
): CallParseResult | null {
  // pos points at '('
  let cursor = pos + 1
  const args: unknown[] = []
  const argWarnings: string[] = []
  const unresolvedArgShapes: string[] = []
  cursor = skipSpaces(text, cursor)

  if (text[cursor] === ')') {
    return { args, argWarnings, unresolvedArgShapes, end: cursor + 1, aborted: false }
  }

  while (cursor < text.length) {
    cursor = skipSpaces(text, cursor)
    const ch = text[cursor]
    if (ch === undefined) return null

    if (ch === '$') {
      const m = tryParseMacro(text, cursor, ctx)
      if (!m) return null // bad macro form mid-arg
      args.push(m.value)
      if (m.value === undefined) {
        unresolvedArgShapes.push(m.shape)
      }
      cursor = m.end
    } else if (ch === "'" || ch === '"') {
      const quote = ch
      const close = text.indexOf(quote, cursor + 1)
      if (close < 0) return null
      args.push(text.slice(cursor + 1, close))
      cursor = close + 1
    } else if (ch === '-' || (ch >= '0' && ch <= '9')) {
      const numRe = /-?\d+(?:\.\d+)?/y
      numRe.lastIndex = cursor
      const m = numRe.exec(text)
      if (!m) return null
      args.push(Number(m[0]))
      cursor = numRe.lastIndex
    } else {
      // Unsupported arg form (identifier, parenthesis, etc.)
      return null
    }

    cursor = skipSpaces(text, cursor)
    if (text[cursor] === ',') {
      cursor++
      continue
    }
    if (text[cursor] === ')') {
      return {
        args,
        argWarnings,
        unresolvedArgShapes,
        end: cursor + 1,
        aborted: false,
      }
    }
    return null
  }

  return null
}

/**
 * Single-pass template walker. Resolves function calls (registered in
 * `TEMPLATE_FUNCTIONS`) and macros (`$variable`, `$variable.col`,
 * `$cell[N].col`, `$cell.selected.col`, `$row.col`, `$row["col"]`,
 * `$from`/`$to`) in left-to-right order. Returns the rendered text and
 * any warnings collected during the walk.
 */
export function evaluateTemplate(text: string, ctx: EvaluateTemplateCtx): EvaluateTemplateResult {
  const out: string[] = []
  const warningSet = new Set<string>()
  let pos = 0

  const emitWarning = (w: string) => {
    if (!warningSet.has(w)) warningSet.add(w)
  }

  while (pos < text.length) {
    const ch = text[pos]

    // Branch (a): identifier-in-registry followed by '('
    if (ch >= 'a' && ch <= 'z' || ch >= 'A' && ch <= 'Z' || ch === '_') {
      const ident = matchIdent(text, pos)
      if (ident && text[pos + ident.length] === '(' && Object.prototype.hasOwnProperty.call(TEMPLATE_FUNCTIONS, ident)) {
        const parenStart = pos + ident.length
        const parsed = tryParseCallArgs(text, parenStart, ctx)
        if (parsed && !parsed.aborted) {
          const callSource = text.slice(pos, parsed.end)
          if (parsed.unresolvedArgShapes.length > 0) {
            for (const s of parsed.unresolvedArgShapes) {
              emitWarning(`${ident}: ${s} is unresolved`)
            }
            out.push(callSource)
            pos = parsed.end
            continue
          }
          const fn = TEMPLATE_FUNCTIONS[ident]
          const result = fn(parsed.args)
          if (result === undefined) {
            // Distinguish arity vs. coercion failures with a generic message.
            if (parsed.args.length !== expectedArity(ident)) {
              emitWarning(
                `${ident}: expected ${expectedArity(ident)} arguments, got ${parsed.args.length}`,
              )
            } else {
              emitWarning(`${ident}: invalid argument value`)
            }
            out.push(callSource)
            pos = parsed.end
            continue
          }
          out.push(result)
          pos = parsed.end
          continue
        }
        // Parse aborted → fall through to literal copy.
      }
      // Not a call → copy one char and continue.
      out.push(ch)
      pos++
      continue
    }

    // Branch (b): macro
    if (ch === '$') {
      const m = tryParseMacro(text, pos, ctx)
      if (m) {
        if (m.value === undefined) {
          emitWarning(`${m.shape} is unresolved`)
          out.push(m.source)
        } else {
          out.push(formatArrowValue(m.value, m.dataType))
        }
        pos = m.end
        continue
      }
      // No shape matched → literal '$', advance one.
      out.push('$')
      pos++
      continue
    }

    // Branch (c): literal copy
    out.push(ch)
    pos++
  }

  return { text: out.join(''), warnings: [...warningSet] }
}

/** Expected arity for diagnostic messages. Keep in sync with registry. */
function expectedArity(name: string): number {
  if (name === 'format_value') return 2
  return -1
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
