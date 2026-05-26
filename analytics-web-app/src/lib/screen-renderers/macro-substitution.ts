/**
 * Legacy SQL-targeted macro substitution.
 *
 * Pre-`evaluateTemplate` macro engine kept for SQL queries (where its
 * single-quote-doubling escape is exactly what's needed). Markdown
 * templates use `evaluateTemplate` from `./template-evaluator` instead.
 *
 * Shapes recognized, in resolution order:
 *   1. `$from` / `$to`
 *   2. `$cell[N].col`        (cell result row reference)
 *   3. `$cell.selected.col`  (selected row reference)
 *   4. `$variable.col`       (multi-column variable column access)
 *   5. `$variable`           (simple variable)
 */

import type { Table, DataType } from 'apache-arrow'
import { isTimeType, timestampToDate } from '@/lib/arrow-utils'
import type { VariableValue } from './notebook-types'
import { getVariableString } from './notebook-types'

/**
 * Format an Arrow value as a string, converting timestamps to RFC3339.
 * Shared with `template-evaluator.ts` for naked macro emission.
 */
export function formatArrowValue(value: unknown, dataType?: DataType): string {
  if (dataType && isTimeType(dataType)) {
    const date = timestampToDate(value, dataType)
    if (date) return date.toISOString()
  }
  return String(value)
}

function escapeSqlValue(value: string): string {
  return value.replace(/'/g, "''")
}

// Each factory returns a fresh RegExp with the 'g' flag so callers get
// independent `lastIndex` state. Defined once to avoid drift between
// `substituteMacrosImpl`, `validateMacros`, and `findUnresolvedSelectionMacro`.

/** $cell[N].col — cell result row reference. */
export const cellRefRegex = () => /\$([a-zA-Z_][a-zA-Z0-9_]*)\[(\d+)\]\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g

/** $cell.selected.col — selected row reference. */
export const selectedRefRegex = () => /\$([a-zA-Z_][a-zA-Z0-9_]*)\.selected\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g

/** $variable.col — dotted variable reference. */
export const dottedVarRegex = () => /\$([a-zA-Z_][a-zA-Z0-9_]*)\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g

/** $variable (not followed by . or [) — simple variable reference. */
export const simpleVarRegex = () => /\$([a-zA-Z_][a-zA-Z0-9_]*)\b(?![.[])/g

/**
 * Substitutes macros in SQL with variable values and time range.
 * - $from and $to are replaced with timestamp values (user controls quoting)
 * - $variable.column accesses specific columns in multi-column variables
 * - $variable accesses the first column value (or the string value for simple variables)
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

  // 1. $from / $to
  result = result.replace(/\$from\b/g, escape(timeRange.begin))
  result = result.replace(/\$to\b/g, escape(timeRange.end))

  // 2. $cell[N].col — must run before dotted/simple variables to avoid partial matches.
  result = result.replace(cellRefRegex(), (match, cellName, rowIdxStr, colName) => {
    const table = cellResults[cellName]
    if (!table) return match
    const rowIdx = parseInt(rowIdxStr, 10)
    if (rowIdx >= table.numRows) return match
    const row = table.get(rowIdx)
    if (!row || row[colName] === undefined || row[colName] === null) return match
    const field = table.schema.fields.find((f) => f.name === colName)
    return escape(formatArrowValue(row[colName], field?.type))
  })

  // 2b. $cell.selected.col — must run before dotted variables. Empty string for unresolved.
  result = result.replace(selectedRefRegex(), (_match, cellName, colName) => {
    const selection = cellSelections[cellName]
    if (!selection) return ''
    const value = selection[colName]
    if (value === undefined || value === null) return ''
    const table = cellResults[cellName]
    const field = table?.schema.fields.find((f) => f.name === colName)
    return escape(formatArrowValue(value, field?.type))
  })

  // 3. $variable.col — must run before simple variables.
  result = result.replace(dottedVarRegex(), (match, varName, colName) => {
    const value = variables[varName]
    if (value === undefined) return match
    if (typeof value === 'string') return match // simple variable has no columns; leave unresolved
    const colValue = value[colName]
    if (colValue === undefined) return match
    return escape(colValue)
  })

  // 4. $variable — sorted by name length descending to avoid partial matches
  //    ($metric vs $metric_name). Lookahead avoids matching $variable.column.
  const sortedVars = Object.entries(variables).sort((a, b) => b[0].length - a[0].length)
  for (const [name, value] of sortedVars) {
    const regex = new RegExp(`\\$${name}\\b(?![.\\[])`, 'g')
    if (typeof value === 'string') {
      result = result.replace(regex, escape(value))
    } else {
      result = result.replace(regex, escape(getVariableString(value)))
    }
  }

  return result
}

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

  // $cell[N].col
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
        const columns = table.schema.fields.map((f) => f.name)
        if (!columns.includes(colName)) {
          errors.push(`Column '${colName}' not found in cell '${cellName}'. Available: ${columns.join(', ')}`)
        }
      }
    }
  }

  // $cell.selected.col
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

  // Collect selected-ref cell names so the dotted-var pass can skip them.
  const selectedRefCellNames = new Set<string>()
  const selectedRefScan = selectedRefRegex()
  while ((match = selectedRefScan.exec(text)) !== null) {
    selectedRefCellNames.add(match[1])
  }

  // $variable.col (skip $cell.selected.* handled above)
  const dottedPattern = dottedVarRegex()
  while ((match = dottedPattern.exec(text)) !== null) {
    const [, varName, colName] = match
    if (colName === 'selected' && selectedRefCellNames.has(varName)) continue
    const value = variables[varName]
    if (value === undefined) {
      errors.push(`Unknown variable: ${varName}`)
    } else if (typeof value === 'string') {
      errors.push(`Variable '${varName}' is not a multi-column variable, cannot access '${colName}'`)
    } else if (value[colName] === undefined) {
      errors.push(
        `Column '${colName}' not found in variable '${varName}'. Available: ${Object.keys(value).join(', ')}`,
      )
    }
  }

  // $variable
  const simplePattern = simpleVarRegex()
  while ((match = simplePattern.exec(text)) !== null) {
    const [, varName] = match
    if (varName === 'from' || varName === 'to' || varName === 'order_by') continue
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
