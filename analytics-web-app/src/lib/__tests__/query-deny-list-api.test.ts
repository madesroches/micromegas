import { tableFromArrays } from 'apache-arrow'
import {
  buildDenyQueriesSql,
  buildListQueryDenialsSql,
  buildRemoveQueryDenialSql,
  decodeQueryDenyRules,
  escapeSqlLiteral,
  extractRemoveResult,
  extractRuleId,
  formatRelativeTime,
} from '../query-deny-list-api'

describe('escapeSqlLiteral', () => {
  it('doubles a single quote', () => {
    expect(escapeSqlLiteral("O'Brien")).toBe("O''Brien")
  })

  it('is a no-op on a string with no quotes', () => {
    expect(escapeSqlLiteral('client = grafana')).toBe('client = grafana')
  })

  it('escapes every quote in a string with several', () => {
    expect(escapeSqlLiteral("a'b'c")).toBe("a''b''c")
  })
})

describe('buildDenyQueriesSql', () => {
  it('escapes a reason containing a single quote exactly once', () => {
    const sql = buildDenyQueriesSql("client = 'grafana'", "it's a test")
    // Exactly one escaped pair for the reason's one quote -- not escaped twice, and not left
    // unescaped (which would break out of the literal).
    expect(sql).toContain("'it''s a test'")
    expect(sql).not.toContain("it''''s")
  })

  it('round-trips a single-quoted expression through escape unchanged in meaning', () => {
    // The textarea holds the expression in its natural, single-quoted form -- e.g. this is
    // exactly what the mockup shows in the match-expression field.
    const matchExpr = "sql_hash = '9f2c41ab73de0155' AND entrypoint = 'grafana-alert'"
    const sql = buildDenyQueriesSql(matchExpr, 'alert re-firing')
    // The doubled quotes decode back to the original literal value server-side (the same rule
    // `substitute_macros` follows): '' inside a SQL string literal means one literal quote.
    const innerLiteral = sql.match(/deny_queries\('(.*)', '.*'\)/)?.[1]
    expect(innerLiteral).toBe(
      "sql_hash = ''9f2c41ab73de0155'' AND entrypoint = ''grafana-alert''"
    )
    // Which is exactly matchExpr with every ' doubled.
    expect(innerLiteral).toBe(matchExpr.replace(/'/g, "''"))
  })

  it('includes the AS rule_id alias so the result can be decoded uniformly', () => {
    const sql = buildDenyQueriesSql("client = 'x'", 'r')
    expect(sql).toContain('AS rule_id')
    expect(sql).toContain('FROM deny_queries(')
  })
})

describe('buildRemoveQueryDenialSql', () => {
  it('escapes the rule id and aliases the result', () => {
    const sql = buildRemoveQueryDenialSql("abc'; drop table x; --")
    expect(sql).toBe("SELECT remove_query_denial('abc''; drop table x; --') AS result")
  })
})

describe('buildListQueryDenialsSql', () => {
  it('selects every documented column from list_query_denials()', () => {
    const sql = buildListQueryDenialsSql()
    expect(sql).toContain('FROM list_query_denials()')
    for (const col of ['rule_id', 'created_at', 'created_by', 'reason', 'match_expr', 'last_hit_at']) {
      expect(sql).toContain(col)
    }
  })
})

describe('decodeQueryDenyRules', () => {
  it('decodes rows, including a null last_hit_at', () => {
    const table = tableFromArrays({
      rule_id: ['11111111-1111-1111-1111-111111111111'],
      created_at: [BigInt(1_700_000_000) * BigInt(1_000_000_000)],
      created_by: ['admin@example.com'],
      reason: ['test rule'],
      match_expr: ["client = 'grafana'"],
      last_hit_at: [null],
    })
    const rows = decodeQueryDenyRules(table)
    expect(rows).toHaveLength(1)
    expect(rows[0].ruleId).toBe('11111111-1111-1111-1111-111111111111')
    expect(rows[0].createdBy).toBe('admin@example.com')
    expect(rows[0].reason).toBe('test rule')
    expect(rows[0].matchExpr).toBe("client = 'grafana'")
    expect(rows[0].lastHitAt).toBeNull()
    expect(rows[0].createdAt).not.toBeNull()
  })

  it('decodes an empty table to an empty array', () => {
    const table = tableFromArrays({
      rule_id: [] as string[],
      created_at: [] as bigint[],
      created_by: [] as string[],
      reason: [] as string[],
      match_expr: [] as string[],
      last_hit_at: [] as (bigint | null)[],
    })
    expect(decodeQueryDenyRules(table)).toEqual([])
  })
})

describe('extractRuleId', () => {
  it('reads the rule_id column from a single-row result', () => {
    const table = tableFromArrays({ rule_id: ['22222222-2222-2222-2222-222222222222'] })
    expect(extractRuleId(table)).toBe('22222222-2222-2222-2222-222222222222')
  })

  it('returns null for an empty result', () => {
    const table = tableFromArrays({ rule_id: [] as string[] })
    expect(extractRuleId(table)).toBeNull()
  })
})

describe('extractRemoveResult', () => {
  it('reads the result column', () => {
    const table = tableFromArrays({ result: ['SUCCESS: removed rule 1234'] })
    expect(extractRemoveResult(table)).toBe('SUCCESS: removed rule 1234')
  })
})

describe('formatRelativeTime', () => {
  const now = new Date('2026-01-01T00:00:00Z')

  it('renders "never" for null', () => {
    expect(formatRelativeTime(null, now)).toBe('never')
  })

  it('renders seconds for a very recent hit', () => {
    expect(formatRelativeTime(new Date('2025-12-31T23:59:56Z'), now)).toBe('4 s ago')
  })

  it('renders weeks for an old hit', () => {
    expect(formatRelativeTime(new Date('2025-12-11T00:00:00Z'), now)).toBe('3 weeks ago')
  })
})
