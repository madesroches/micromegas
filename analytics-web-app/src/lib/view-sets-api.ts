/**
 * SQL builders and Arrow -> row decoding for the Admin -> View Sets screen.
 *
 * The screen is a front end for `list_view_set_definitions()`, `list_view_sets()`,
 * `list_partitions()`, `check_view_merge(ddl)` and the `CREATE [OR REPLACE]` / `DROP
 * MATERIALIZED VIEW` DDL, all issued with `adminMode: true`. No REST routes of its own.
 */
import type { Table } from 'apache-arrow'
import { timestampToDate } from './arrow-utils'
import { escapeSqlLiteral } from './sql-literal'

export type ViewSetHealth = 'ok' | 'failed to load' | 'read-only'

export interface ViewSetDefinition {
  name: string
  definitionSql: string | null
  updateGroup: number | null
  updatedAt: Date | null
  updatedBy: string | null
  source: string
}

export interface LoadedViewSet {
  name: string
  description: string | null
  columns: string | null
  sortOrder: string | null
}

export interface ViewSetColumn {
  name: string
  type: string
}

export interface ViewSetRow {
  name: string
  source: string
  health: ViewSetHealth
  definitionSql: string | null
  description: string | null
  columns: ViewSetColumn[]
  sortOrder: string[]
  updatedAt: Date | null
  updatedBy: string | null
}

export interface PartitionStats {
  viewSetName: string
  partitionCount: number
  fileSize: number
  numRows: number
  beginInsertTime: Date | null
  endInsertTime: Date | null
  lastUpdated: Date | null
}

export interface MergeCheck {
  streams: boolean
  sortExecLine: string | null
}

export const CREATE_SKELETON = `CREATE MATERIALIZED VIEW my_view_set WITH (
  extract_query = $$
    SELECT date_bin('1 minute', time) as time_bin, target, count(*) as count,
           arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience
      FROM log_entries
     WHERE insert_time >= '{begin}' AND insert_time < '{end}'
     GROUP BY time_bin, target, audience
  $$,
  count_src_query = $$
    SELECT sum(nb_objects) as count FROM blocks
     WHERE insert_time >= '{begin}' AND insert_time < '{end}'
  $$,
  merge_partitions_query = $$
    SELECT time_bin, target, sum(count) as count, audience
      FROM {source} GROUP BY time_bin, target, audience
  $$,
  update_group           = 4000,
  time_column            = 'time_bin',
  source_partition_delta = '1 day',
  merge_partition_delta  = '1 day',
  merge_sort_order       = 'time_bin, target'
)`

export function buildListViewSetDefinitionsSql(): string {
  return (
    'SELECT view_set_name, definition_sql, update_group, updated_at, updated_by, source ' +
    'FROM list_view_set_definitions()'
  )
}

export function buildListViewSetsSql(): string {
  return 'SELECT view_set_name, description, columns, sort_order FROM list_view_sets()'
}

export function buildPartitionStatsSql(): string {
  return (
    'SELECT view_set_name, count(*) AS partition_count, sum(file_size) AS file_size, ' +
    'sum(num_rows) AS num_rows, min(begin_insert_time) AS begin_insert_time, ' +
    'max(end_insert_time) AS end_insert_time, max(updated) AS updated ' +
    'FROM list_partitions() GROUP BY view_set_name'
  )
}

export function buildCheckViewMergeSql(ddl: string): string {
  return `SELECT streams, sort_exec_line FROM check_view_merge('${escapeSqlLiteral(ddl)}')`
}

/** Identifiers are not escaped: a name that is not a plain identifier is rejected server-side. */
export function buildDropSql(name: string): string {
  return `DROP MATERIALIZED VIEW ${name}`
}

const CREATE_HEADER = /^CREATE(\s+OR\s+REPLACE)?(\s+MATERIALIZED\s+VIEW\b)/i

function skipLeadingTrivia(sql: string): number | null {
  let i = 0
  for (;;) {
    while (i < sql.length && /\s/.test(sql[i])) i++
    if (sql.startsWith('--', i)) {
      const nl = sql.indexOf('\n', i)
      i = nl === -1 ? sql.length : nl + 1
    } else if (sql.startsWith('/*', i)) {
      const end = sql.indexOf('*/', i + 2)
      if (end === -1) return null
      i = end + 2
    } else {
      return i
    }
  }
}

/**
 * Rewrites the statement header to `CREATE OR REPLACE MATERIALIZED VIEW`. The header is matched
 * only where the statement actually starts (after comments), so an unterminated block comment
 * that happens to contain a `CREATE ... MATERIALIZED VIEW` is left alone.
 */
export function toReplaceStatement(sql: string): string {
  const start = skipLeadingTrivia(sql)
  if (start === null) return sql
  const match = CREATE_HEADER.exec(sql.slice(start))
  if (!match || match[1]) return sql
  return (
    sql.slice(0, start) +
    'CREATE OR REPLACE' +
    match[2] +
    sql.slice(start + match[0].length)
  )
}

const VIEW_NAME = /^(?:CREATE(?:\s+OR\s+REPLACE)?\s+MATERIALIZED\s+VIEW)\s+("(?:[^"]|"")*"|[^\s("]+)/i

/** Name the statement's header gives the view set, with quotes removed; null if there is none. */
export function extractViewSetName(sql: string): string | null {
  const start = skipLeadingTrivia(sql)
  if (start === null) return null
  const match = VIEW_NAME.exec(sql.slice(start))
  if (!match) return null
  const raw = match[1]
  return raw.startsWith('"') ? raw.slice(1, -1).replace(/""/g, '"') : raw
}

function optString(value: unknown): string | null {
  return value == null ? null : String(value)
}

export function decodeViewSetDefinitions(table: Table): ViewSetDefinition[] {
  const updatedAtType = table.schema.fields.find((f) => f.name === 'updated_at')?.type
  const rows: ViewSetDefinition[] = []
  for (let i = 0; i < table.numRows; i++) {
    const row = table.get(i)
    if (!row) continue
    rows.push({
      name: String(row.view_set_name ?? ''),
      definitionSql: optString(row.definition_sql),
      updateGroup: row.update_group == null ? null : Number(row.update_group),
      updatedAt: row.updated_at == null ? null : timestampToDate(row.updated_at, updatedAtType),
      updatedBy: optString(row.updated_by),
      source: String(row.source ?? ''),
    })
  }
  return rows
}

export function decodeLoadedViewSets(table: Table): LoadedViewSet[] {
  const rows: LoadedViewSet[] = []
  for (let i = 0; i < table.numRows; i++) {
    const row = table.get(i)
    if (!row) continue
    rows.push({
      name: String(row.view_set_name ?? ''),
      description: optString(row.description),
      columns: optString(row.columns),
      sortOrder: optString(row.sort_order),
    })
  }
  return rows
}

export function decodePartitionStats(table: Table): PartitionStats[] {
  const typeOf = (name: string) => table.schema.fields.find((f) => f.name === name)?.type
  const date = (value: unknown, name: string) =>
    value == null ? null : timestampToDate(value, typeOf(name))
  const rows: PartitionStats[] = []
  for (let i = 0; i < table.numRows; i++) {
    const row = table.get(i)
    if (!row) continue
    rows.push({
      viewSetName: String(row.view_set_name ?? ''),
      partitionCount: Number(row.partition_count ?? 0),
      fileSize: Number(row.file_size ?? 0),
      numRows: Number(row.num_rows ?? 0),
      beginInsertTime: date(row.begin_insert_time, 'begin_insert_time'),
      endInsertTime: date(row.end_insert_time, 'end_insert_time'),
      lastUpdated: date(row.updated, 'updated'),
    })
  }
  return rows
}

/** `streams` is false when the merge buffers its whole input; no row means nothing to check. */
export function decodeMergeCheck(table: Table): MergeCheck | null {
  if (table.numRows === 0) return null
  const row = table.get(0)
  if (!row) return null
  return { streams: Boolean(row.streams), sortExecLine: optString(row.sort_exec_line) }
}

/** `columns` is one `name: DataType` line per column; split on the first `": "` only. */
export function parseColumns(columns: string | null): ViewSetColumn[] {
  if (!columns) return []
  return columns
    .split('\n')
    .filter((line) => line.length > 0)
    .map((line) => {
      const sep = line.indexOf(': ')
      return sep === -1
        ? { name: line, type: '' }
        : { name: line.slice(0, sep), type: line.slice(sep + 2) }
    })
}

export function parseSortOrder(sortOrder: string | null): string[] {
  return sortOrder ? sortOrder.split(', ') : []
}

/**
 * Joins the stored definitions with the loaded view sets. A `ddl` definition absent from the
 * loaded sets failed to load; built-ins have no stored definition and are read-only.
 */
export function mergeViewSetRows(
  definitions: ViewSetDefinition[],
  loaded: LoadedViewSet[]
): ViewSetRow[] {
  const loadedByName = new Map(loaded.map((l) => [l.name, l]))
  return definitions.map((def) => {
    const live = loadedByName.get(def.name)
    const health: ViewSetHealth =
      def.source === 'builtin' ? 'read-only' : live ? 'ok' : 'failed to load'
    return {
      name: def.name,
      source: def.source,
      health,
      definitionSql: def.definitionSql,
      description: live?.description ?? null,
      columns: parseColumns(live?.columns ?? null),
      sortOrder: parseSortOrder(live?.sortOrder ?? null),
      updatedAt: def.updatedAt,
      updatedBy: def.updatedBy,
    }
  })
}
