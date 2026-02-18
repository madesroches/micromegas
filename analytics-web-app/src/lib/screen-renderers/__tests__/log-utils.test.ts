import {
  LEVEL_NAMES,
  formatLocalTime,
  getLevelColor,
  formatLevelValue,
  classifyLogColumns,
} from '../log-utils'

// Mock arrow-utils (timestampToDate) used by formatLocalTime
jest.mock('@/lib/arrow-utils', () => ({
  timestampToDate: (value: unknown) => {
    if (!value) return null
    if (value instanceof Date) return value
    const date = new Date(String(value))
    return isNaN(date.getTime()) ? null : date
  },
}))

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
  it('classifies all four known columns in canonical order', () => {
    // Schema has them in non-canonical order
    const fields = [mockField('msg'), mockField('time', 'timestamp'), mockField('level'), mockField('target')]

    const columns = classifyLogColumns(fields as never)

    expect(columns.map((c) => c.name)).toEqual(['time', 'level', 'target', 'msg'])
    expect(columns.map((c) => c.kind)).toEqual(['time', 'level', 'target', 'msg'])
  })

  it('appends extra columns after known columns in schema order', () => {
    const fields = [
      mockField('time', 'timestamp'),
      mockField('process_id'),
      mockField('level'),
      mockField('thread_id'),
      mockField('msg'),
    ]

    const columns = classifyLogColumns(fields as never)

    expect(columns.map((c) => c.name)).toEqual(['time', 'level', 'msg', 'process_id', 'thread_id'])
    expect(columns[3].kind).toBe('generic')
    expect(columns[4].kind).toBe('generic')
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
    expect(columns.map((c) => c.kind)).toEqual(['time', 'msg'])
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
