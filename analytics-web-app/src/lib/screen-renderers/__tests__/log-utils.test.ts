import {
  Field,
  Int32,
  List,
  Struct,
  Table,
  Timestamp,
  TimeUnit,
  Utf8,
  vectorFromArray,
} from 'apache-arrow'
import {
  LEVEL_NAMES,
  formatLocalTime,
  getLevelColor,
  formatLevelValue,
  classifyLogColumns,
  formatLogValue,
  formatRowForCopy,
  computeFlexWidths,
  groupRepeatedRows,
  COLLAPSE_WINDOW_ROWS,
  range,
  singletonGroups,
  type LogColumn,
} from '../log-utils'

// Mock arrow-utils (timestampToDate) used by formatLocalTime
// Mock timestampToDate only — formatCell (via formatLogValue's generic branch)
// also needs the rest of arrow-utils' real exports (isTimeType, isDurationType, etc.).
vi.mock('@/lib/arrow-utils', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/arrow-utils')>()
  return {
    ...actual,
    timestampToDate: (value: unknown) => {
      if (!value) return null
      if (value instanceof Date) return value
      const date = new Date(String(value))
      return isNaN(date.getTime()) ? null : date
    },
  }
})

// =============================================================================
// LEVEL_NAMES
// =============================================================================

describe('LEVEL_NAMES', () => {
  it('maps numeric levels 1-6 to standard names', () => {
    expect(LEVEL_NAMES[1]).toBe('FATAL')
    expect(LEVEL_NAMES[2]).toBe('ERROR')
    expect(LEVEL_NAMES[3]).toBe('WARN')
    expect(LEVEL_NAMES[4]).toBe('INFO')
    expect(LEVEL_NAMES[5]).toBe('DEBUG')
    expect(LEVEL_NAMES[6]).toBe('TRACE')
  })

  it('returns undefined for unknown level numbers', () => {
    expect(LEVEL_NAMES[0]).toBeUndefined()
    expect(LEVEL_NAMES[7]).toBeUndefined()
  })
})

// =============================================================================
// getLevelColor
// =============================================================================

describe('getLevelColor', () => {
  it('returns distinct classes for each standard level', () => {
    expect(getLevelColor('FATAL')).toBe('text-accent-error-bright')
    expect(getLevelColor('ERROR')).toBe('text-accent-error')
    expect(getLevelColor('WARN')).toBe('text-accent-warning')
    expect(getLevelColor('INFO')).toBe('text-accent-link')
    expect(getLevelColor('DEBUG')).toBe('text-theme-text-secondary')
    expect(getLevelColor('TRACE')).toBe('text-theme-text-muted')
  })

  it('returns primary text color for unknown levels', () => {
    expect(getLevelColor('UNKNOWN')).toBe('text-theme-text-primary')
    expect(getLevelColor('')).toBe('text-theme-text-primary')
  })
})

// =============================================================================
// formatLevelValue
// =============================================================================

describe('formatLevelValue', () => {
  it('converts numeric level to name', () => {
    expect(formatLevelValue(4)).toBe('INFO')
    expect(formatLevelValue(2)).toBe('ERROR')
  })

  it('returns UNKNOWN for out-of-range numbers', () => {
    expect(formatLevelValue(0)).toBe('UNKNOWN')
    expect(formatLevelValue(99)).toBe('UNKNOWN')
  })

  it('passes through string values', () => {
    expect(formatLevelValue('WARN')).toBe('WARN')
    expect(formatLevelValue('custom')).toBe('custom')
  })

  it('handles null and undefined', () => {
    expect(formatLevelValue(null)).toBe('')
    expect(formatLevelValue(undefined)).toBe('')
  })
})

// =============================================================================
// formatLocalTime
// =============================================================================

describe('formatLocalTime', () => {
  it('returns padded empty string for falsy input', () => {
    expect(formatLocalTime(null)).toHaveLength(29)
    expect(formatLocalTime(undefined)).toHaveLength(29)
    expect(formatLocalTime('')).toHaveLength(29)
  })

  it('formats a date string with nanosecond precision', () => {
    const result = formatLocalTime('2024-01-15T10:30:45.123456789Z')
    // Should contain nanoseconds from the string
    expect(result).toContain('123456789')
    // Should have the date portion (local time, so just check format shape)
    expect(result).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{9}$/)
  })

  it('pads short fractional seconds to 9 digits', () => {
    const result = formatLocalTime('2024-01-15T10:30:45.12Z')
    expect(result).toContain('120000000')
  })

  it('uses zeros when no fractional seconds', () => {
    const result = formatLocalTime('2024-01-15T10:30:45Z')
    expect(result).toContain('000000000')
  })

  it('returns padded empty for unparseable values', () => {
    expect(formatLocalTime('not-a-date')).toHaveLength(29)
  })
})

// =============================================================================
// classifyLogColumns
// =============================================================================

// Helper to create mock Field objects
function mockField(name: string, typeId = 'utf8'): { name: string; type: { typeId: string } } {
  return { name, type: { typeId } }
}

describe('classifyLogColumns', () => {
  it('preserves schema order and classifies known columns', () => {
    const fields = [
      mockField('msg'),
      mockField('time', 'timestamp'),
      mockField('level'),
      mockField('target'),
    ]

    const columns = classifyLogColumns(fields as never)

    expect(columns.map((c) => c.name)).toEqual(['msg', 'time', 'level', 'target'])
    expect(columns.map((c) => c.kind)).toEqual(['generic', 'time', 'level', 'target'])
  })

  it('preserves schema order with mixed known and extra columns', () => {
    const fields = [
      mockField('time', 'timestamp'),
      mockField('process_id'),
      mockField('level'),
      mockField('thread_id'),
      mockField('msg'),
    ]

    const columns = classifyLogColumns(fields as never)

    expect(columns.map((c) => c.name)).toEqual(['time', 'process_id', 'level', 'thread_id', 'msg'])
    expect(columns.map((c) => c.kind)).toEqual(['time', 'generic', 'level', 'generic', 'generic'])
  })

  it('handles schema with only extra columns (no known columns)', () => {
    const fields = [mockField('count'), mockField('avg_duration')]

    const columns = classifyLogColumns(fields as never)

    expect(columns).toHaveLength(2)
    expect(columns.every((c) => c.kind === 'generic')).toBe(true)
    expect(columns.map((c) => c.name)).toEqual(['count', 'avg_duration'])
  })

  it('handles schema with subset of known columns', () => {
    const fields = [mockField('time', 'timestamp'), mockField('msg')]

    const columns = classifyLogColumns(fields as never)

    expect(columns.map((c) => c.name)).toEqual(['time', 'msg'])
    expect(columns.map((c) => c.kind)).toEqual(['time', 'generic'])
  })

  it('handles empty schema', () => {
    expect(classifyLogColumns([])).toEqual([])
  })

  it('preserves the Field type on each column', () => {
    const fields = [mockField('time', 'timestamp'), mockField('extra', 'int32')]

    const columns = classifyLogColumns(fields as never)

    expect((columns[0].type as unknown as { typeId: string }).typeId).toBe('timestamp')
    expect((columns[1].type as unknown as { typeId: string }).typeId).toBe('int32')
  })
})

// =============================================================================
// formatLogValue
// =============================================================================

describe('formatLogValue', () => {
  it('formats a time value via formatLocalTime', () => {
    const col: LogColumn = {
      name: 'time',
      kind: 'time',
      type: new Timestamp(TimeUnit.MILLISECOND, null),
    }
    expect(formatLogValue(col, '2024-01-15T10:30:45Z')).toMatch(
      /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{9}$/,
    )
  })

  it('formats a numeric level value via LEVEL_NAMES', () => {
    const col: LogColumn = { name: 'level', kind: 'level', type: new Int32() }
    expect(formatLogValue(col, 3)).toBe('WARN')
  })

  it('formats a string level value by passthrough', () => {
    const col: LogColumn = { name: 'level', kind: 'level', type: new Utf8() }
    expect(formatLogValue(col, 'custom')).toBe('custom')
  })

  it('formats a target value as a plain string', () => {
    const col: LogColumn = { name: 'target', kind: 'target', type: new Utf8() }
    expect(formatLogValue(col, 'my::module::path')).toBe('my::module::path')
  })

  it('formats a generic value via formatCell', () => {
    const col: LogColumn = { name: 'msg', kind: 'generic', type: new Utf8() }
    expect(formatLogValue(col, 'hello')).toBe('hello')
  })
})

// =============================================================================
// formatRowForCopy — characterization tests guarding the formatLogValue refactor
// =============================================================================

describe('formatRowForCopy', () => {
  const columns: LogColumn[] = [
    {
      name: 'time',
      kind: 'time',
      type: new Timestamp(TimeUnit.MILLISECOND, null),
    },
    { name: 'level', kind: 'level', type: new Int32() },
    { name: 'target', kind: 'target', type: new Utf8() },
    { name: 'msg', kind: 'generic', type: new Utf8() },
  ]

  it('formats one tab-delimited value per column, in column order', () => {
    const row = {
      time: '2024-01-15T10:30:45Z',
      level: 4,
      target: 'my::mod',
      msg: 'hello',
    }
    const parts = formatRowForCopy(columns, row).split('\t')
    expect(parts).toHaveLength(4)
    expect(parts[1]).toBe('INFO')
    expect(parts[2]).toBe('my::mod')
    expect(parts[3]).toBe('hello')
  })

  it('replaces embedded tabs and newlines with a space so the copy stays tab-delimited', () => {
    const row = {
      time: '2024-01-15T10:30:45Z',
      level: 4,
      target: 'my::mod',
      msg: 'line1\nline2\twith-tab',
    }
    const result = formatRowForCopy(columns, row)
    expect(result.endsWith('line1 line2 with-tab')).toBe(true)
    expect(result).not.toMatch(/[\n\r]/)
  })
})

// =============================================================================
// computeFlexWidths — characterization tests guarding the formatLogValue refactor
// =============================================================================

function makeFlexTable(rows: Record<string, unknown>[]) {
  return {
    numRows: rows.length,
    get: (i: number) => rows[i] ?? null,
  }
}

describe('computeFlexWidths', () => {
  it('clamps a short level value up to MIN_LEVEL_WIDTH_PX (40px)', () => {
    const columns: LogColumn[] = [{ name: 'level', kind: 'level', type: new Utf8() }]
    const table = makeFlexTable([{ level: 'x' }])
    expect(computeFlexWidths(table, columns, [0]).level).toBe(40)
  })

  it('clamps a long target value down to 200px', () => {
    const columns: LogColumn[] = [{ name: 'target', kind: 'target', type: new Utf8() }]
    const table = makeFlexTable([{ target: 'x'.repeat(100) }])
    expect(computeFlexWidths(table, columns, [0]).target).toBe(200)
  })

  it('clamps a long generic value down to MAX_FLEX_WIDTH_PX (700px)', () => {
    const columns: LogColumn[] = [{ name: 'msg', kind: 'generic', type: new Utf8() }]
    const table = makeFlexTable([{ msg: 'x'.repeat(200) }])
    expect(computeFlexWidths(table, columns, [0]).msg).toBe(700)
  })

  it('clamps an empty generic value up to MIN_FLEX_WIDTH_PX (60px)', () => {
    const columns: LogColumn[] = [{ name: 'msg', kind: 'generic', type: new Utf8() }]
    const table = makeFlexTable([{ msg: '' }])
    expect(computeFlexWidths(table, columns, [0]).msg).toBe(60)
  })

  it('a time column always measures the fixed 29-char format (209px)', () => {
    const columns: LogColumn[] = [
      {
        name: 'time',
        kind: 'time',
        type: new Timestamp(TimeUnit.MILLISECOND, null),
      },
    ]
    const table = makeFlexTable([{ time: '2024-01-15T10:30:45Z' }])
    expect(computeFlexWidths(table, columns, [0]).time).toBe(209)
  })

  it('only measures the rows passed in, not the whole table', () => {
    const columns: LogColumn[] = [{ name: 'msg', kind: 'generic', type: new Utf8() }]
    const table = makeFlexTable([{ msg: 'short' }, { msg: 'x'.repeat(200) }])
    expect(computeFlexWidths(table, columns, [0]).msg).toBeLessThan(700)
  })
})

// =============================================================================
// groupRepeatedRows
// =============================================================================

function buildLogTable(
  rows: {
    time: number
    level: number
    target: string
    msg: string
    request_id?: string
  }[],
): Table {
  return new Table({
    time: vectorFromArray(
      rows.map((r) => r.time),
      new Timestamp(TimeUnit.MILLISECOND, null),
    ),
    level: vectorFromArray(
      rows.map((r) => r.level),
      new Int32(),
    ),
    target: vectorFromArray(
      rows.map((r) => r.target),
      new Utf8(),
    ),
    msg: vectorFromArray(
      rows.map((r) => r.msg),
      new Utf8(),
    ),
    request_id: vectorFromArray(
      rows.map((r) => r.request_id ?? ''),
      new Utf8(),
    ),
  })
}

const g = (rows: number[], anchor: number) => ({ rows, anchor })

describe('groupRepeatedRows', () => {
  const timeIgnored = new Set(['time'])
  const rowsOf = (msgs: string[], times?: number[]) =>
    buildLogTable(
      msgs.map((msg, i) => ({
        time: times ? times[i] : i + 1,
        level: 4,
        target: 't',
        msg,
      })),
    )
  const run = (table: Table, ignore: ReadonlySet<string> = timeIgnored) =>
    groupRepeatedRows(table, classifyLogColumns(table.schema.fields), ignore)

  it('groups rows differing only in time; a differing msg is a separate group', () => {
    expect(run(rowsOf(['same', 'same', 'different']))).toMatchObject([g([0, 1], 1), g([2], 2)])
  })

  it('merges interleaved messages: A,B,A,B,A,B gives two groups of 3 ordered by anchor', () => {
    expect(run(rowsOf(['A', 'B', 'A', 'B', 'A', 'B']))).toMatchObject([g([0, 2, 4], 4), g([1, 3, 5], 5)])
  })

  it('groups A,B,A as A over rows [0,2] and B alone', () => {
    expect(run(rowsOf(['A', 'B', 'A']))).toMatchObject([g([1], 1), g([0, 2], 2)])
  })

  it('merges at exactly the window limit and splits beyond it', () => {
    const fill = (gap: number) => [
      'A',
      ...Array(gap)
        .fill(null)
        .map((_, i) => `x${i}`),
      'A',
    ]
    const atLimit = run(rowsOf(fill(COLLAPSE_WINDOW_ROWS - 1)))
    expect(atLimit.find((x) => x.rows.length > 1)?.rows).toMatchObject([0, COLLAPSE_WINDOW_ROWS])
    const beyond = run(rowsOf(fill(COLLAPSE_WINDOW_ROWS)))
    expect(beyond.every((x) => x.rows.length === 1)).toBe(true)
  })

  it('measures the window from the latest member, so a long run keeps merging', () => {
    const msgs: string[] = []
    for (let i = 0; i < 3; i++) {
      msgs.push('A')
      for (let j = 0; j < 150; j++) msgs.push(`x${i}_${j}`)
    }
    const merged = run(rowsOf(msgs)).find((x) => x.rows.length > 1)
    expect(merged?.rows).toMatchObject([0, 151, 302])
  })

  it('anchors at the latest-by-time member for ascending and descending results', () => {
    expect(run(rowsOf(['A', 'A', 'A'], [1, 2, 3]))).toMatchObject([g([0, 1, 2], 2)])
    expect(run(rowsOf(['A', 'A', 'A'], [3, 2, 1]))).toMatchObject([g([0, 1, 2], 0)])
  })

  it('records earliest and latest time by value regardless of row order', () => {
    const [grp] = run(rowsOf(['A', 'A', 'A'], [2, 3, 1]))
    expect(grp.firstTime).toBe(1)
    expect(grp.lastTime).toBe(3)
  })

  it('anchors at the last row when there is no time column', () => {
    const table = new Table({
      msg: vectorFromArray(['A', 'B', 'A'], new Utf8()),
    })
    expect(run(table, new Set())).toMatchObject([g([1], 1), g([0, 2], 2)])
  })

  it('groups rows differing only in an ignored column, but not when that column is compared', () => {
    const table = buildLogTable([
      { time: 1, level: 4, target: 't', msg: 'same', request_id: 'a' },
      { time: 2, level: 4, target: 't', msg: 'same', request_id: 'b' },
    ])
    expect(run(table, new Set(['time', 'request_id']))).toMatchObject([g([0, 1], 1)])
    expect(run(table)).toMatchObject([g([0], 0), g([1], 1)])
  })

  it('falls back to singleton groups when every column is ignored', () => {
    const table = rowsOf(['same', 'same'])
    const ignore = new Set(classifyLogColumns(table.schema.fields).map((c) => c.name))
    expect(run(table, ignore)).toMatchObject(singletonGroups(2))
  })

  it('treats null equal to null, but not equal to an empty string', () => {
    const table = new Table({
      msg: vectorFromArray([null, '', null], new Utf8()),
    })
    expect(run(table, new Set())).toMatchObject(
      [g([0, 2], 2), g([1], 1)].sort((a, b) => a.anchor - b.anchor),
    )
  })

  it('does not confuse undefined with null', () => {
    const cols: LogColumn[] = [{ name: 'msg', kind: 'generic', type: new Utf8() }]
    const values = [undefined, null, undefined]
    const table = {
      numRows: 3,
      getChild: () => ({ get: (i: number) => values[i] }),
    }
    expect(groupRepeatedRows(table, cols, new Set())).toMatchObject([g([1], 1), g([0, 2], 2)])
  })

  it('groups bigint values without throwing, and keeps them apart from the same digits as a string', () => {
    const cols: LogColumn[] = [{ name: 'v', kind: 'generic', type: new Utf8() }]
    const values: unknown[] = [10n, '10', 10n]
    const table = {
      numRows: 3,
      getChild: () => ({ get: (i: number) => values[i] }),
    }
    expect(groupRepeatedRows(table, cols, new Set())).toMatchObject([g([1], 1), g([0, 2], 2)])
  })

  it('cannot collide a value containing the separator with a different tuple', () => {
    const table = new Table({
      a: vectorFromArray(['x","sy', 'x'], new Utf8()),
      b: vectorFromArray(['z', 'y","sz'], new Utf8()),
    })
    expect(run(table, new Set())).toMatchObject([g([0], 0), g([1], 1)])
  })

  it('treats an object-valued column (list) as equal when its formatted value matches', () => {
    const tagsType = new List(new Field('item', new Utf8()))
    const table = new Table({
      tags: vectorFromArray(
        [
          ['a', 'b'],
          ['a', 'c'],
          ['a', 'b'],
        ],
        tagsType,
      ),
    })
    expect(run(table, new Set())).toMatchObject([g([1], 1), g([0, 2], 2)])
  })

  it('treats an object-valued column (struct) as equal when its formatted value matches', () => {
    const structType = new Struct([new Field('a', new Int32()), new Field('b', new Utf8())])
    const table = new Table({
      s: vectorFromArray(
        [
          { a: 1, b: 'x' },
          { a: 2, b: 'y' },
          { a: 1, b: 'x' },
        ],
        structType,
      ),
    })
    expect(run(table, new Set())).toMatchObject([g([1], 1), g([0, 2], 2)])
  })

  it('returns [] for an empty table', () => {
    expect(run(buildLogTable([]))).toMatchObject([])
  })

  it('returns one group for a single row', () => {
    expect(run(rowsOf(['only']))).toMatchObject([g([0], 0)])
  })
})

// =============================================================================
// range / singletonGroups
// =============================================================================

describe('range', () => {
  it('returns [start, end) as an array', () => {
    expect(range(2, 5)).toMatchObject([2, 3, 4])
  })

  it('returns an empty array when start === end', () => {
    expect(range(3, 3)).toMatchObject([])
  })
})

describe('singletonGroups', () => {
  it('returns one group per row', () => {
    expect(singletonGroups(3)).toMatchObject([g([0], 0), g([1], 1), g([2], 2)])
  })

  it('returns [] for zero rows', () => {
    expect(singletonGroups(0)).toMatchObject([])
  })
})
