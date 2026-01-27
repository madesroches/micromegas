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
} from './notebook-types'

// Reserved URL parameter names that cannot be used as variable names
const RESERVED_URL_PARAMS = new Set(['from', 'to', 'type'])

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
 * Deep comparison of two notebook configs using JSON serialization.
 * Works because configs are JSON-serializable by design.
 */
export function notebookConfigsEqual(
  a: import('./notebook-types').NotebookConfig | null,
  b: import('./notebook-types').NotebookConfig | null
): boolean {
  if (a === b) return true
  if (!a || !b) return false
  return JSON.stringify(a) === JSON.stringify(b)
}

/**
 * Substitutes macros in SQL with variable values and time range.
 * - $begin and $end are replaced with quoted timestamps
 * - User variables are replaced without quotes (SQL author controls quoting)
 * - Single quotes in values are escaped for SQL safety
 */
export function substituteMacros(
  sql: string,
  variables: Record<string, string>,
  timeRange: { begin: string; end: string }
): string {
  let result = sql
  // Substitute $begin and $end (these are timestamps, keep quotes)
  result = result.replace(/\$begin/g, `'${timeRange.begin}'`)
  result = result.replace(/\$end/g, `'${timeRange.end}'`)
  // Substitute user variables - don't add quotes, let the SQL author control quoting
  // Sort by name length descending to avoid partial matches ($metric vs $metric_name)
  const sortedVars = Object.entries(variables).sort((a, b) => b[0].length - a[0].length)
  for (const [name, value] of sortedVars) {
    const regex = new RegExp(`\\$${name}\\b`, 'g')
    // Escape single quotes in value for SQL safety
    const escaped = value.replace(/'/g, "''")
    result = result.replace(regex, escaped)
  }
  return result
}
