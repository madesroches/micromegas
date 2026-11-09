import { tableFromArrays } from 'apache-arrow'
import {
  buildCheckViewMergeSql,
  buildDropSql,
  buildListViewSetDefinitionsSql,
  buildListViewSetsSql,
  buildPartitionStatsSql,
  decodeLoadedViewSets,
  decodeMergeCheck,
  decodePartitionStats,
  decodeViewSetDefinitions,
  extractViewSetName,
  mergeViewSetRows,
  parseColumns,
  parseSortOrder,
  toReplaceStatement,
} from '../view-sets-api'

describe('SQL builders', () => {
  it('lists definitions and loaded sets with explicit columns', () => {
    expect(buildListViewSetDefinitionsSql()).toContain('FROM list_view_set_definitions()')
    expect(buildListViewSetsSql()).toContain('FROM list_view_sets()')
  })

  it('aggregates partitions per view set server-side', () => {
    const sql = buildPartitionStatsSql()
    expect(sql).toContain('FROM list_partitions() GROUP BY view_set_name')
    expect(sql).toContain('count(*)')
  })

  it('escapes quotes in the DDL passed to check_view_merge and leaves $$ alone', () => {
    const ddl = "CREATE MATERIALIZED VIEW v WITH (extract_query = $$ SELECT 'a' $$)"
    expect(buildCheckViewMergeSql(ddl)).toBe(
      "SELECT streams, sort_exec_line FROM check_view_merge('CREATE MATERIALIZED VIEW v WITH (extract_query = $$ SELECT ''a'' $$)')"
    )
  })

  it('builds a DROP without IF EXISTS', () => {
    expect(buildDropSql('my_view')).toBe('DROP MATERIALIZED VIEW my_view')
  })
})

describe('toReplaceStatement', () => {
  it('rewrites a plain CREATE', () => {
    expect(toReplaceStatement('CREATE MATERIALIZED VIEW v WITH (a = 1)')).toBe(
      'CREATE OR REPLACE MATERIALIZED VIEW v WITH (a = 1)'
    )
  })

  it('leaves an existing OR REPLACE alone', () => {
    const sql = 'CREATE OR REPLACE MATERIALIZED VIEW v WITH (a = 1)'
    expect(toReplaceStatement(sql)).toBe(sql)
  })

  it('skips leading line and block comments', () => {
    expect(toReplaceStatement('-- note\n/* more */\nCREATE MATERIALIZED VIEW v')).toBe(
      '-- note\n/* more */\nCREATE OR REPLACE MATERIALIZED VIEW v'
    )
  })

  it('leaves text with an unterminated block comment untouched', () => {
    const sql = '/* CREATE MATERIALIZED VIEW v'
    expect(toReplaceStatement(sql)).toBe(sql)
  })

  it('handles lowercase keywords', () => {
    expect(toReplaceStatement('create materialized view v')).toBe(
      'CREATE OR REPLACE materialized view v'
    )
  })

  it('leaves other statements untouched', () => {
    expect(toReplaceStatement('SELECT 1')).toBe('SELECT 1')
  })
})

describe('extractViewSetName', () => {
  it('reads plain, and OR REPLACE names after leading comments', () => {
    expect(extractViewSetName('CREATE MATERIALIZED VIEW v WITH (a = 1)')).toBe('v')
    expect(extractViewSetName('-- c\n/* x */ create or replace materialized view v(a)')).toBe('v')
  })

  it('returns null when there is no CREATE MATERIALIZED VIEW header', () => {
    expect(extractViewSetName('SELECT 1')).toBeNull()
    expect(extractViewSetName('/* CREATE MATERIALIZED VIEW v')).toBeNull()
  })
})

describe('decoders', () => {
  it('decodes definitions with nullable definition_sql', () => {
    const table = tableFromArrays({
      view_set_name: ['log_stats', 'log_entries'],
      definition_sql: ['CREATE MATERIALIZED VIEW log_stats', null],
      update_group: [4000, null],
      updated_at: [BigInt(1_700_000_000) * BigInt(1_000_000_000), null],
      updated_by: ['admin', null],
      source: ['ddl', 'builtin'],
    })
    const rows = decodeViewSetDefinitions(table)
    expect(rows).toHaveLength(2)
    expect(rows[0].definitionSql).toBe('CREATE MATERIALIZED VIEW log_stats')
    expect(rows[0].updatedBy).toBe('admin')
    expect(rows[1].definitionSql).toBeNull()
    expect(rows[1].updatedAt).toBeNull()
    expect(rows[1].source).toBe('builtin')
  })

  it('decodes loaded sets with nullable description and sort_order', () => {
    const table = tableFromArrays({
      view_set_name: ['a', 'b'],
      description: ['# Usage', null],
      columns: ['x: Int64\ny: Utf8', null],
      sort_order: ['time, target', null],
    })
    const rows = decodeLoadedViewSets(table)
    expect(rows[0]).toEqual({
      name: 'a',
      description: '# Usage',
      columns: 'x: Int64\ny: Utf8',
      sortOrder: 'time, target',
    })
    expect(rows[1].description).toBeNull()
    expect(rows[1].sortOrder).toBeNull()
  })

  it('decodes partition stats', () => {
    const table = tableFromArrays({
      view_set_name: ['a'],
      partition_count: [BigInt(3)],
      file_size: [BigInt(2048)],
      num_rows: [BigInt(100)],
      begin_insert_time: [BigInt(1_700_000_000) * BigInt(1_000_000_000)],
      end_insert_time: [BigInt(1_700_000_100) * BigInt(1_000_000_000)],
      updated: [BigInt(1_700_000_200) * BigInt(1_000_000_000)],
    })
    const [stats] = decodePartitionStats(table)
    expect(stats.partitionCount).toBe(3)
    expect(stats.fileSize).toBe(2048)
    expect(stats.numRows).toBe(100)
    expect(stats.beginInsertTime).toBeInstanceOf(Date)
  })

  it('decodes the merge check, and null for no rows', () => {
    const table = tableFromArrays({ streams: [false], sort_exec_line: ['SortExec: expr=[t]'] })
    expect(decodeMergeCheck(table)).toEqual({ streams: false, sortExecLine: 'SortExec: expr=[t]' })
    const empty = tableFromArrays({ streams: [] as boolean[], sort_exec_line: [] as string[] })
    expect(decodeMergeCheck(empty)).toBeNull()
  })
})

describe('parseColumns / parseSortOrder', () => {
  it('splits on newline, then on the first ": " only', () => {
    expect(parseColumns('a: Int64\nb: Dictionary(Int32, Utf8)\nc: Struct(x: Int8)')).toEqual([
      { name: 'a', type: 'Int64' },
      { name: 'b', type: 'Dictionary(Int32, Utf8)' },
      { name: 'c', type: 'Struct(x: Int8)' },
    ])
  })

  it('returns no columns for null', () => {
    expect(parseColumns(null)).toEqual([])
  })

  it('splits sort_order on ", " and treats null as empty', () => {
    expect(parseSortOrder('time_bin, target')).toEqual(['time_bin', 'target'])
    expect(parseSortOrder('time')).toEqual(['time'])
    expect(parseSortOrder(null)).toEqual([])
  })
})

describe('mergeViewSetRows', () => {
  const def = (name: string, source: string) => ({
    name,
    definitionSql: source === 'ddl' ? 'CREATE MATERIALIZED VIEW x' : null,
    updateGroup: null,
    updatedAt: null,
    updatedBy: null,
    source,
  })

  it('marks a ddl definition missing from the loaded sets as failed to load', () => {
    const rows = mergeViewSetRows([def('a', 'ddl'), def('b', 'ddl')], [
      { name: 'a', description: 'd', columns: 'x: Int64', sortOrder: 'x' },
    ])
    expect(rows[0].health).toBe('ok')
    expect(rows[0].columns).toEqual([{ name: 'x', type: 'Int64' }])
    expect(rows[1].health).toBe('failed to load')
    expect(rows[1].columns).toEqual([])
  })

  it('marks built-ins read-only', () => {
    const rows = mergeViewSetRows([def('log_entries', 'builtin')], [
      { name: 'log_entries', description: null, columns: null, sortOrder: null },
    ])
    expect(rows[0].health).toBe('read-only')
    expect(rows[0].definitionSql).toBeNull()
  })
})
