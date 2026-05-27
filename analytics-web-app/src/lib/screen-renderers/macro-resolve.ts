/**
 * Shared macro value lookup.
 *
 * Both macro engines — the regex sweep in `./macro-substitution` and the
 * single-pass walker in `./template-evaluator` — keep their own *parsing*
 * strategy but route every value lookup through `resolveMacro`. Given an
 * already-parsed `MacroSpan`, it returns the raw value, a `resolved` flag,
 * and the source column's `DataType`. Callers own formatting, escaping, and
 * what an unresolved macro looks like in their output.
 *
 * No formatting/escaping happens here — raw values only.
 */

import type { Table, DataType } from 'apache-arrow'
import type { VariableValue } from './notebook-types'
import { getVariableString } from './notebook-types'

/** A parsed macro shape, independent of the engine that parsed it. */
export type MacroSpan =
  | { kind: 'time'; which: 'from' | 'to' }
  | { kind: 'cellRow'; cell: string; rowIdx: number; col: string }
  | { kind: 'selected'; cell: string; col: string }
  | { kind: 'rowCol'; col: string } // $row.col and $row["col"]
  | { kind: 'varCol'; name: string; col: string }
  | { kind: 'var'; name: string } // $variable, or bare $col when bareColumnsFromRow

/** Everything any macro shape might need to resolve. Superset of the old
 *  EvaluateTemplateCtx; the SQL path supplies only the first four fields. */
export interface ResolveCtx {
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
  /** When true, a bare `$ident` resolves to `row[ident]` (with its column
   *  DataType) before falling back to a notebook variable. Map detail
   *  templates set this so `$col` means "the selected row's col"; the
   *  table-override path leaves it false and addresses columns via `$row.col`. */
  bareColumnsFromRow?: boolean
}

export interface ResolvedMacro {
  /** Raw JS/Arrow value when resolved; undefined otherwise. */
  value: unknown
  resolved: boolean
  /** Arrow DataType of the value's source column, when known. Lets callers
   *  pick the right formatter (RFC3339 for timestamps) and detect time types. */
  dataType?: DataType
}

const UNRESOLVED: ResolvedMacro = { value: undefined, resolved: false }

function fieldType(table: Table | undefined, col: string): DataType | undefined {
  return table?.schema.fields.find((f) => f.name === col)?.type
}

/**
 * Resolve a parsed macro shape against the available context.
 *
 * Each branch reproduces the union of both engines' lookup logic exactly;
 * the engines only differ in how each treats `resolved: false`, which stays
 * with the callers.
 */
export function resolveMacro(span: MacroSpan, ctx: ResolveCtx): ResolvedMacro {
  switch (span.kind) {
    case 'time': {
      if (!ctx.timeRange) return UNRESOLVED
      const value = span.which === 'from' ? ctx.timeRange.begin : ctx.timeRange.end
      return { value, resolved: true }
    }

    case 'cellRow': {
      const table = ctx.cellResults[span.cell]
      if (!table || span.rowIdx >= table.numRows) return UNRESOLVED
      const row = table.get(span.rowIdx)
      if (!row || row[span.col] === undefined || row[span.col] === null) return UNRESOLVED
      return { value: row[span.col], resolved: true, dataType: fieldType(table, span.col) }
    }

    case 'selected': {
      const selection = ctx.cellSelections[span.cell]
      if (!selection) return UNRESOLVED
      const value = selection[span.col]
      if (value === undefined || value === null) return UNRESOLVED
      return { value, resolved: true, dataType: fieldType(ctx.cellResults[span.cell], span.col) }
    }

    case 'rowCol': {
      if (ctx.row === undefined) return UNRESOLVED
      const value = ctx.row[span.col]
      if (value === undefined || value === null) return UNRESOLVED
      return { value, resolved: true, dataType: ctx.columnTypes?.get(span.col) }
    }

    case 'varCol': {
      const variable = ctx.variables[span.name]
      if (variable === undefined || typeof variable === 'string') return UNRESOLVED
      const colValue = variable[span.col]
      if (colValue === undefined) return UNRESOLVED
      return { value: colValue, resolved: true }
    }

    case 'var': {
      // Map detail templates address the selected row's columns as bare `$col`,
      // with columns winning name collisions against notebook variables.
      if (ctx.bareColumnsFromRow && ctx.row !== undefined) {
        const value = ctx.row[span.name]
        if (value !== undefined && value !== null) {
          return { value, resolved: true, dataType: ctx.columnTypes?.get(span.name) }
        }
      }
      const variable = ctx.variables[span.name]
      if (variable === undefined) return UNRESOLVED
      const value = typeof variable === 'string' ? variable : getVariableString(variable)
      return { value, resolved: true }
    }
  }
}
