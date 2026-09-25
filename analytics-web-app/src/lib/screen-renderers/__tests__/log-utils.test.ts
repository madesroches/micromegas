import { Field, Int32, List, Struct, Table, Timestamp, TimeUnit, Utf8, vectorFromArray } from 'apache-arrow'
import {
  LEVEL_NAMES,
  formatLocalTime,
  getLevelColor,
  formatLevelValue,
  classifyLogColumns,
  formatLogValue,
  formatRowForCopy,
  computeFlexWidths,
  groupConsecutiveRows,
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
    const fields = [mockField('msg'), mockField('time', 'timestamp'), mockField('level'), mockField('target')]

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
    const col: LogColumn = { name: 'time', kind: 'time', type: new Timestamp(TimeUnit.MILLISECOND, null) }
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
    { name: 'time', kind: 'time', type: new Timestamp(TimeUnit.MILLISECOND, null) },
    { name: 'level', kind: 'level', type: new Int32() },
    { name: 'target', kind: 'target', type: new Utf8() },
    { name: 'msg', kind: 'generic', type: new Utf8() },
  ]

  it('formats one tab-delimited value per column, in column order', () => {
    const row = { time: '2024-01-15T10:30:45Z', level: 4, target: 'my::mod', msg: 'hello' }
    const parts = formatRowForCopy(columns, row).split('\t')
    expect(parts).toHaveLength(4)
    expect(parts[1]).toBe('INFO')
    expect(parts[2]).toBe('my::mod')
    expect(parts[3]).toBe('hello')
  })

  it('replaces embedded tabs and newlines with a space so the copy stays tab-delimited', () => {
    const row = { time: '2024-01-15T10:30:45Z', level: 4, target: 'my::mod', msg: 'line1\nline2\twith-tab' }
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
    const columns: LogColumn[] = [{ name: 'time', kind: 'time', type: new Timestamp(TimeUnit.MILLISECOND, null) }]
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
// groupConsecutiveRows
// =============================================================================

function buildLogTable(
  rows: { time: number; level: number; target: string; msg: string; request_id?: string }[],
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

describe('groupConsecutiveRows', () => {
  it('groups rows differing only in time; a differing msg breaks the run', () => {
    const table = buildLogTable([
      { time: 1, level: 4, target: 't', msg: 'same' },
      { time: 2, level: 4, target: 't', msg: 'same' },
      { time: 3, level: 4, target: 't', msg: 'different' },
    ])
    const columns = classifyLogColumns(table.schema.fields)
    const groups = groupConsecutiveRows(table, columns, new Set(['time']))
    expect(groups).toEqual([
      { start: 0, end: 2 },
      { start: 2, end: 3 },
    ])
  })

  it('keeps non-consecutive duplicates (A, B, A) as three separate groups', () => {
    const table = buildLogTable([
      { time: 1, level: 4, target: 't', msg: 'A' },
      { time: 2, level: 4, target: 't', msg: 'B' },
      { time: 3, level: 4, target: 't', msg: 'A' },
    ])
    const columns = classifyLogColumns(table.schema.fields)
    const groups = groupConsecutiveRows(table, columns, new Set(['time']))
    expect(groups).toEqual([
      { start: 0, end: 1 },
      { start: 1, end: 2 },
      { start: 2, end: 3 },
    ])
  })

  it('groups rows differing only in an ignored column, but not when that column is compared', () => {
    const table = buildLogTable([
      { time: 1, level: 4, target: 't', msg: 'same', request_id: 'a' },
      { time: 2, level: 4, target: 't', msg: 'same', request_id: 'b' },
    ])
    const columns = classifyLogColumns(table.schema.fields)

    const grouped = groupConsecutiveRows(table, columns, new Set(['time', 'request_id']))
    expect(grouped).toEqual([{ start: 0, end: 2 }])

    const notGrouped = groupConsecutiveRows(table, columns, new Set(['time']))
    expect(notGrouped).toEqual([
      { start: 0, end: 1 },
      { start: 1, end: 2 },
    ])
  })

  it('falls back to singleton groups when every column is ignored', () => {
    const table = buildLogTable([
      { time: 1, level: 4, target: 't', msg: 'same' },
      { time: 2, level: 4, target: 't', msg: 'same' },
    ])
    const columns = classifyLogColumns(table.schema.fields)
    const ignore = new Set(columns.map((c) => c.name))
    expect(groupConsecutiveRows(table, columns, ignore)).toEqual(singletonGroups(2))
  })

  it('treats null equal to null, but not equal to an empty string', () => {
    const table = new Table({ msg: vectorFromArray([null, null, ''], new Utf8()) })
    const columns = classifyLogColumns(table.schema.fields)
    const groups = groupConsecutiveRows(table, columns, new Set())
    expect(groups).toEqual([
      { start: 0, end: 2 },
      { start: 2, end: 3 },
    ])
  })

  it('treats an object-valued column (list) as equal when its formatted value matches', () => {
    const tagsType = new List(new Field('item', new Utf8()))
    const table = new Table({
      tags: vectorFromArray(
        [
          ['a', 'b'],
          ['a', 'b'],
          ['a', 'c'],
        ],
        tagsType,
      ),
    })
    const columns = classifyLogColumns(table.schema.fields)
    const groups = groupConsecutiveRows(table, columns, new Set())
    expect(groups).toEqual([
      { start: 0, end: 2 },
      { start: 2, end: 3 },
    ])
  })

  it('treats an object-valued column (struct) as equal when its formatted value matches', () => {
    const structType = new Struct([new Field('a', new Int32()), new Field('b', new Utf8())])
    const table = new Table({
      s: vectorFromArray(
        [
          { a: 1, b: 'x' },
          { a: 1, b: 'x' },
          { a: 2, b: 'y' },
        ],
        structType,
      ),
    })
    const columns = classifyLogColumns(table.schema.fields)
    const groups = groupConsecutiveRows(table, columns, new Set())
    expect(groups).toEqual([
      { start: 0, end: 2 },
      { start: 2, end: 3 },
    ])
  })

  it('returns [] for an empty table', () => {
    const table = buildLogTable([])
    const columns = classifyLogColumns(table.schema.fields)
    expect(groupConsecutiveRows(table, columns, new Set(['time']))).toEqual([])
  })

  it('returns one group for a single row', () => {
    const table = buildLogTable([{ time: 1, level: 4, target: 't', msg: 'only' }])
    const columns = classifyLogColumns(table.schema.fields)
    expect(groupConsecutiveRows(table, columns, new Set(['time']))).toEqual([{ start: 0, end: 1 }])
  })
})

// =============================================================================
// range / singletonGroups
// =============================================================================

describe('range', () => {
  it('returns [start, end) as an array', () => {
    expect(range(2, 5)).toEqual([2, 3, 4])
  })

  it('returns an empty array when start === end', () => {
    expect(range(3, 3)).toEqual([])
  })
})

describe('singletonGroups', () => {
  it('returns one group per row', () => {
    expect(singletonGroups(3)).toEqual([
      { start: 0, end: 1 },
      { start: 1, end: 2 },
      { start: 2, end: 3 },
    ])
  })

  it('returns [] for zero rows', () => {
    expect(singletonGroups(0)).toEqual([])
  })
})
