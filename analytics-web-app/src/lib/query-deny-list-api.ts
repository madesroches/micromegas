/**
 * SQL builders and Arrow -> row decoding for the Admin -> Query Deny List screen
 * (`tasks/query_deny_list_plan.md` §9).
 *
 * The screen is a front end for three SQL functions -- `list_query_denials()`,
 * `deny_queries(match_expr, reason)`, `remove_query_denial(rule_id)` -- issued through the same
 * `useStreamQuery` -> `/api/stream-query` path every other SQL-driven page uses, against
 * whichever data source the admin has selected. No REST routes of its own.
 */
import type { Table } from 'apache-arrow'
import { timestampToDate } from './arrow-utils'

export interface QueryDenyRule {
  ruleId: string
  createdAt: Date | null
  createdBy: string
  reason: string
  matchExpr: string
  lastHitAt: Date | null
}

/**
 * Doubles a single quote inside a SQL string literal -- the same escaping rule
 * `substitute_macros` already follows server-side (`analytics-web-srv/src/stream_query.rs`).
 * Apply this at the one place a value is embedded into a SQL statement; never build one of the
 * statements below any other way.
 */
export function escapeSqlLiteral(value: string): string {
  return value.replace(/'/g, "''")
}

/** `SELECT * FROM list_query_denials()`, with an explicit column list matching {@link decodeQueryDenyRules}. */
export function buildListQueryDenialsSql(): string {
  return (
    'SELECT rule_id, created_at, created_by, reason, match_expr, last_hit_at ' +
    'FROM list_query_denials()'
  )
}

/**
 * `SELECT * FROM deny_queries('<expr>', '<reason>')`. `deny_queries` is backed by the same
 * (time, msg) log-stream shape every mutating admin UDTF uses; `msg` carries the new rule's id,
 * aliased here to `rule_id` so {@link extractRuleId} has a stable column name to read.
 */
export function buildDenyQueriesSql(matchExpr: string, reason: string): string {
  return `SELECT msg AS rule_id FROM deny_queries('${escapeSqlLiteral(matchExpr)}', '${escapeSqlLiteral(reason)}')`
}

/** `SELECT remove_query_denial('<rule_id>') AS result`. */
export function buildRemoveQueryDenialSql(ruleId: string): string {
  return `SELECT remove_query_denial('${escapeSqlLiteral(ruleId)}') AS result`
}

function columnType(table: Table, name: string) {
  return table.schema.fields.find((f) => f.name === name)?.type
}

/** Decodes `list_query_denials()`'s result into one {@link QueryDenyRule} per row. */
export function decodeQueryDenyRules(table: Table): QueryDenyRule[] {
  const createdAtType = columnType(table, 'created_at')
  const lastHitAtType = columnType(table, 'last_hit_at')
  const rows: QueryDenyRule[] = []
  for (let i = 0; i < table.numRows; i++) {
    const row = table.get(i)
    if (!row) continue
    const lastHitAt = row.last_hit_at
    rows.push({
      ruleId: String(row.rule_id ?? ''),
      createdAt: timestampToDate(row.created_at, createdAtType),
      createdBy: String(row.created_by ?? ''),
      reason: String(row.reason ?? ''),
      matchExpr: String(row.match_expr ?? ''),
      lastHitAt: lastHitAt == null ? null : timestampToDate(lastHitAt, lastHitAtType),
    })
  }
  return rows
}

/** Reads the new rule's id out of `deny_queries`'s single-row result. */
export function extractRuleId(table: Table): string | null {
  if (table.numRows === 0) return null
  const row = table.get(0)
  const ruleId = row ? String(row.rule_id ?? '') : ''
  return ruleId || null
}

/** Reads `remove_query_denial`'s status string out of its single-row result. */
export function extractRemoveResult(table: Table): string | null {
  if (table.numRows === 0) return null
  const row = table.get(0)
  const result = row ? String(row.result ?? '') : ''
  return result || null
}

/** Relative "N ago" rendering for `last_hit_at` -- "4 s ago" reads as "still firing", "3 weeks
 * ago" as "probably removable" (the reading the plan's mockup calls for). `null` means the rule
 * has never fired. */
export function formatRelativeTime(date: Date | null, now: Date = new Date()): string {
  if (!date) return 'never'
  const seconds = Math.max(0, Math.round((now.getTime() - date.getTime()) / 1000))
  if (seconds < 60) return `${seconds} s ago`
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours} h ago`
  const days = Math.round(hours / 24)
  if (days < 7) return `${days} d ago`
  const weeks = Math.round(days / 7)
  return `${weeks} week${weeks === 1 ? '' : 's'} ago`
}
