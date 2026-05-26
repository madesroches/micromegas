/**
 * Single-pass template evaluator with function-call support.
 *
 * `evaluateTemplate` walks the template once, left-to-right, dispatching at
 * each position to one of three branches:
 *   (a) identifier-in-registry followed by '(' → function call
 *   (b) '$' → macro shape (variable, cell ref, row ref, $from/$to)
 *   (c) literal char copy
 *
 * Unresolved calls and unresolved macros are left as their original source
 * text — there is no half-substituted state to undo — and each problem is
 * recorded as a warning. The SQL-targeted `substituteMacros` path lives
 * separately in `./macro-substitution` and is unaffected.
 */

import type { Table, DataType } from 'apache-arrow'
import { isTimeType } from '@/lib/arrow-utils'
import { TEMPLATE_FUNCTIONS } from '@/lib/template-functions'
import type { VariableValue } from './notebook-types'
import { getVariableString } from './notebook-types'
import { formatArrowValue } from './macro-substitution'

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
  /** When true, a bare `$ident` resolves to `row[ident]` (with its column
   *  DataType) before falling back to a notebook variable. Map detail
   *  templates set this so `$col` means "the selected row's col"; the
   *  table-override path leaves it false and addresses columns via `$row.col`. */
  bareColumnsFromRow?: boolean
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

function isIdentChar(ch: string): boolean {
  return /[a-zA-Z0-9_]/.test(ch)
}

function tryParseMacro(text: string, pos: number, ctx: EvaluateTemplateCtx): MacroMatch | null {
  if (text[pos] !== '$') return null

  // 1. $from / $to — only recognized when ctx.timeRange is set; otherwise let
  //    them fall through so the walker copies '$' verbatim.
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

  // 2. $cell[N].col OR $row["col"] / $row['col']
  if (text[afterIdent] === '[') {
    const close = text.indexOf(']', afterIdent + 1)
    if (close < 0) return null
    const inside = text.slice(afterIdent + 1, close)

    if (/^\d+$/.test(inside)) {
      // $cell[N].col
      if (text[close + 1] !== '.') return null
      const colMatch = matchIdent(text, close + 2)
      if (!colMatch) return null
      const end = close + 2 + colMatch.length
      const source = text.slice(pos, end)
      const shape = `$${ident}[${inside}].${colMatch}`
      const table = ctx.cellResults[ident]
      const rowIdx = parseInt(inside, 10)
      let value: unknown
      let dataType: DataType | undefined
      if (table && rowIdx < table.numRows) {
        const row = table.get(rowIdx)
        if (row && row[colMatch] !== undefined && row[colMatch] !== null) {
          value = row[colMatch]
          dataType = table.schema.fields.find((f) => f.name === colMatch)?.type
        }
      }
      return { source, end, shape, value, dataType }
    }

    // $row["col"] / $row['col']
    if (ident === 'row' && ctx.row !== undefined) {
      const m = /^(["'])([^"']+)\1$/.exec(inside)
      if (!m) return null
      const colName = m[2]
      const end = close + 1
      const source = text.slice(pos, end)
      const shape = `$row[${inside}]`
      const v = ctx.row[colName]
      let value: unknown
      let dataType: DataType | undefined
      if (v !== undefined && v !== null) {
        value = v
        dataType = ctx.columnTypes?.get(colName)
      }
      return { source, end, shape, value, dataType }
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

    // 4. $row.col (matched BEFORE $variable.col so a row reference is never
    //    shadowed by a varName='row' lookup).
    if (ident === 'row' && ctx.row !== undefined) {
      const end = afterIdent + 1 + second.length
      const source = text.slice(pos, end)
      const shape = `$row.${second}`
      const v = ctx.row[second]
      if (v !== undefined && v !== null) {
        return { source, end, shape, value: v, dataType: ctx.columnTypes?.get(second) }
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

  // 6. $variable  (or $column when bareColumnsFromRow)
  const end = afterIdent
  const source = text.slice(pos, end)
  const shape = `$${ident}`

  // Map detail templates address the selected row's columns as bare `$col`,
  // with columns winning name collisions against notebook variables. Check
  // the row first so the raw value + its DataType reach the caller (enabling
  // RFC3339 emission and precision-preserving format_value).
  if (ctx.bareColumnsFromRow && ctx.row !== undefined) {
    const v = ctx.row[ident]
    if (v !== undefined && v !== null) {
      return { source, end, shape, value: v, dataType: ctx.columnTypes?.get(ident) }
    }
  }

  const variable = ctx.variables[ident]
  let value: unknown
  if (variable !== undefined) {
    value = typeof variable === 'string' ? variable : getVariableString(variable)
  }
  return { source, end, shape, value }
}

interface CallParseResult {
  args: unknown[]
  /** Names of macro shapes that resolved to undefined (for reporting). */
  unresolvedArgShapes: string[]
  /** True when at least one argument was a `$`-prefixed macro. Used as a
   *  conservative heuristic for flagging unknown function names: prose
   *  like "Math.max(1, 2)" parses as a valid call but should not warn. */
  hadMacroArg: boolean
  /** End index in the input (just past the closing ')'). */
  end: number
}

function tryParseCallArgs(
  text: string,
  pos: number,
  ctx: EvaluateTemplateCtx,
): CallParseResult | null {
  // pos points at '('
  let cursor = pos + 1
  const args: unknown[] = []
  const unresolvedArgShapes: string[] = []
  let hadMacroArg = false
  cursor = skipSpaces(text, cursor)

  if (text[cursor] === ')') {
    return { args, unresolvedArgShapes, hadMacroArg, end: cursor + 1 }
  }

  while (cursor < text.length) {
    cursor = skipSpaces(text, cursor)
    const ch = text[cursor]
    if (ch === undefined) return null

    if (ch === '$') {
      const m = tryParseMacro(text, cursor, ctx)
      if (!m) return null
      hadMacroArg = true
      // Time-typed Arrow values (Timestamp/Date/Time) arrive as BigInt
      // epoch counts whose Number-coercion silently loses precision
      // (~1.7e18 ns since epoch). Stringify via formatArrowValue so a
      // numeric template function gets NaN and surfaces a real warning
      // instead of producing nonsense like "53954068.94 years".
      let argValue = m.value
      if (argValue !== undefined && m.dataType && isTimeType(m.dataType)) {
        argValue = formatArrowValue(argValue, m.dataType)
      }
      args.push(argValue)
      if (argValue === undefined) unresolvedArgShapes.push(m.shape)
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
      return null
    }

    cursor = skipSpaces(text, cursor)
    if (text[cursor] === ',') {
      cursor++
      continue
    }
    if (text[cursor] === ')') {
      return { args, unresolvedArgShapes, hadMacroArg, end: cursor + 1 }
    }
    return null
  }

  return null
}

/** Expected arity for diagnostic messages. Keep in sync with `TEMPLATE_FUNCTIONS`. */
function expectedArity(name: string): number {
  if (name === 'format_value') return 2
  return -1
}

/**
 * Resolve template function calls and macros in one left-to-right pass.
 * Returns `{ text, warnings }`. Unresolved spans are copied verbatim.
 */
export function evaluateTemplate(text: string, ctx: EvaluateTemplateCtx): EvaluateTemplateResult {
  const out: string[] = []
  const warningSet = new Set<string>()
  let pos = 0

  const emitWarning = (w: string) => warningSet.add(w)

  while (pos < text.length) {
    const ch = text[pos]

    // Branch (a): identifier followed by '('. We try to parse args for any
    // such call site, then dispatch on whether `ident` is a registered
    // template function. Unknown names are only flagged when at least one
    // arg is a `$`-macro — that filter spares prose like "Math.max(1, 2)".
    if ((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || ch === '_') {
      const ident = matchIdent(text, pos)
      if (ident && text[pos + ident.length] === '(') {
        const parenStart = pos + ident.length
        const parsed = tryParseCallArgs(text, parenStart, ctx)
        if (parsed) {
          const callSource = text.slice(pos, parsed.end)
          const isKnown = Object.prototype.hasOwnProperty.call(TEMPLATE_FUNCTIONS, ident)
          if (!isKnown) {
            if (parsed.hadMacroArg) {
              emitWarning(`Unknown template function: ${ident}`)
              out.push(callSource)
              pos = parsed.end
              continue
            }
            // Fall through to literal copy below.
          } else {
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
        }
        // Parse aborted → fall through to literal copy.
      }
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
