import { CellType } from './cell-registry'

// ============================================================================
// Types
// ============================================================================

export interface CellConfigBase {
  name: string
  type: CellType
  layout: { height: number | 'auto'; collapsed?: boolean }
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
 */
export function validateCellName(
  name: string,
  existingNames: Set<string>,
  currentName?: string
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

  return null
}

/**
 * Creates a default cell configuration for the given type.
 * Generates a unique name if the base name already exists.
 */
export function createDefaultCell(type: CellType, existingNames: Set<string>): CellConfig {
  // Generate unique name (use underscore separator for valid identifiers)
  const baseName = type.charAt(0).toUpperCase() + type.slice(1)
  let name = baseName
  let counter = 1
  while (existingNames.has(name)) {
    counter++
    name = `${baseName}_${counter}`
  }

  const baseConfig: CellConfigBase = {
    name,
    type,
    layout: { height: 'auto' },
  }

  switch (type) {
    case 'table':
    case 'chart':
    case 'log':
      return { ...baseConfig, type, sql: DEFAULT_SQL[type] } as QueryCellConfig
    case 'markdown':
      return { ...baseConfig, type: 'markdown', content: '# Notes\n\nAdd your documentation here.' } as MarkdownCellConfig
    case 'variable':
      return {
        ...baseConfig,
        type: 'variable',
        variableType: 'combobox',
        sql: DEFAULT_SQL.variable,
      } as VariableCellConfig
    default:
      return { ...baseConfig, type: 'table', sql: DEFAULT_SQL.table } as QueryCellConfig
  }
}

/**
 * Notebook config interface for type-safe comparisons.
 */
export interface NotebookConfig {
  cells: CellConfig[]
  refreshInterval?: number
  timeRangeFrom?: string
  timeRangeTo?: string
}

/**
 * Deep comparison of two notebook configs using JSON serialization.
 * Works because configs are JSON-serializable by design.
 */
export function notebookConfigsEqual(a: NotebookConfig | null, b: NotebookConfig | null): boolean {
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
