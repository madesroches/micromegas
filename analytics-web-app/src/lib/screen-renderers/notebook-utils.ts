// Re-export types from notebook-types for backwards compatibility
export type {
  CellType,
  CellStatus,
  CellConfigBase,
  QueryCellConfig,
  MarkdownCellConfig,
  VariableCellConfig,
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

import type { CellConfig, VariableValue } from './notebook-types'
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
  const savedCells = (savedConfig as { cells?: Array<{ type: string; name: string; defaultValue?: string }> }).cells
  if (!savedCells) return

  const keysToDelete: string[] = []
  params.forEach((_value, key) => {
    if (RESERVED_URL_PARAMS.has(key)) return
    const savedCell = savedCells.find((c) => c.type === 'variable' && c.name === key)
    if (savedCell && savedCell.defaultValue === params.get(key)) {
      keysToDelete.push(key)
    }
  })

  for (const key of keysToDelete) {
    params.delete(key)
  }
}

/**
 * Escapes single quotes in a value for SQL safety.
 */
function escapeSqlValue(value: string): string {
  return value.replace(/'/g, "''")
}

/**
 * Substitutes macros in SQL with variable values and time range.
 * - $begin and $end are replaced with timestamp values (user controls quoting)
 * - $variable.column syntax accesses specific columns in multi-column variables
 * - $variable syntax accesses the first column value (or the string value for simple variables)
 * - User variables are replaced without quotes (SQL author controls quoting)
 * - Single quotes in values are escaped for SQL safety
 */
export function substituteMacros(
  sql: string,
  variables: Record<string, VariableValue>,
  timeRange: { begin: string; end: string }
): string {
  let result = sql

  // 1. Substitute $begin and $end (user controls quoting, like other variables)
  result = result.replace(/\$begin\b/g, escapeSqlValue(timeRange.begin))
  result = result.replace(/\$end\b/g, escapeSqlValue(timeRange.end))

  // 2. Handle dotted variable references first: $variable.column
  //    Must process before simple variables to avoid partial matches
  const dottedPattern = /\$([a-zA-Z_][a-zA-Z0-9_]*)\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g
  result = result.replace(dottedPattern, (match, varName, colName) => {
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
    const regex = new RegExp(`\\$${name}\\b(?!\\.)`, 'g')

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
  variables: Record<string, VariableValue>
): MacroValidationResult {
  const errors: string[] = []

  // Check dotted references: $variable.column
  const dottedPattern = /\$([a-zA-Z_][a-zA-Z0-9_]*)\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g
  let match
  while ((match = dottedPattern.exec(text)) !== null) {
    const [, varName, colName] = match
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

  // Check simple variable references: $variable (not followed by a dot)
  const simplePattern = /\$([a-zA-Z_][a-zA-Z0-9_]*)\b(?!\.)/g
  while ((match = simplePattern.exec(text)) !== null) {
    const [, varName] = match
    // Skip built-in variables
    if (varName === 'begin' || varName === 'end' || varName === 'order_by') {
      continue
    }
    if (variables[varName] === undefined) {
      errors.push(`Unknown variable: ${varName}`)
    }
  }

  return { valid: errors.length === 0, errors }
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
