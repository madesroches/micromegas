// Mock matchMedia for uPlot (imported via cell-registry -> ChartCell -> XYChart)
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: jest.fn().mockImplementation((query) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: jest.fn(),
    removeListener: jest.fn(),
    addEventListener: jest.fn(),
    removeEventListener: jest.fn(),
    dispatchEvent: jest.fn(),
  })),
})

// Mock cell-registry to prevent uPlot CSS import chain
// eslint-disable-next-line @typescript-eslint/no-var-requires
jest.mock('../cell-registry', () => require('../__test-utils__/cell-registry-mock').createCellRegistryMock())

import { tableFromArrays, vectorFromArray, Table, Timestamp, TimeUnit } from 'apache-arrow'
import { substituteMacros, DEFAULT_SQL, sanitizeCellName, validateCellName, validateMacros, evaluateTemplate } from '../notebook-utils'
import { serializeVariableValue, deserializeVariableValue, getVariableString, isMultiColumnValue } from '../notebook-types'
import { createDefaultCell } from '../cell-registry'
import { resolveMacro } from '../macro-resolve'
import type { ResolveCtx } from '../macro-resolve'

describe('substituteMacros', () => {
  const defaultTimeRange = { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' }

  describe('time range substitution', () => {
    it('should substitute $from without adding quotes (user controls quoting)', () => {
      const sql = "SELECT * FROM logs WHERE time >= '$from'"
      const result = substituteMacros(sql, {}, defaultTimeRange, {}, {})
      expect(result).toBe("SELECT * FROM logs WHERE time >= '2024-01-01T00:00:00Z'")
    })

    it('should substitute $to without adding quotes (user controls quoting)', () => {
      const sql = "SELECT * FROM logs WHERE time <= '$to'"
      const result = substituteMacros(sql, {}, defaultTimeRange, {}, {})
      expect(result).toBe("SELECT * FROM logs WHERE time <= '2024-01-02T00:00:00Z'")
    })

    it('should substitute both $from and $to', () => {
      const sql = "SELECT * FROM logs WHERE time BETWEEN '$from' AND '$to'"
      const result = substituteMacros(sql, {}, defaultTimeRange, {}, {})
      expect(result).toBe(
        "SELECT * FROM logs WHERE time BETWEEN '2024-01-01T00:00:00Z' AND '2024-01-02T00:00:00Z'"
      )
    })

    it('should substitute multiple occurrences of $from', () => {
      const sql = "SELECT '$from', '$from'"
      const result = substituteMacros(sql, {}, defaultTimeRange, {}, {})
      expect(result).toBe("SELECT '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z'")
    })
  })

  describe('user variable substitution', () => {
    it('should substitute user variables without adding quotes', () => {
      const sql = "SELECT * FROM measures WHERE name = '$metric'"
      const result = substituteMacros(sql, { metric: 'cpu_usage' }, defaultTimeRange, {}, {})
      expect(result).toBe("SELECT * FROM measures WHERE name = 'cpu_usage'")
    })

    it('should substitute variables without quotes when not wrapped', () => {
      const sql = 'SELECT $limit'
      const result = substituteMacros(sql, { limit: '100' }, defaultTimeRange, {}, {})
      expect(result).toBe('SELECT 100')
    })

    it('should escape single quotes in variable values', () => {
      const sql = "SELECT * FROM logs WHERE msg = '$search'"
      const result = substituteMacros(sql, { search: "it's working" }, defaultTimeRange, {}, {})
      expect(result).toBe("SELECT * FROM logs WHERE msg = 'it''s working'")
    })

    it('should escape multiple single quotes in variable values', () => {
      const sql = "SELECT * FROM logs WHERE msg = '$search'"
      const result = substituteMacros(sql, { search: "it's not it's" }, defaultTimeRange, {}, {})
      expect(result).toBe("SELECT * FROM logs WHERE msg = 'it''s not it''s'")
    })

    it('should handle multiple different variables', () => {
      const sql = "SELECT * FROM measures WHERE name = '$metric' AND host = '$host'"
      const result = substituteMacros(
        sql,
        { metric: 'cpu_usage', host: 'server1' },
        defaultTimeRange,
        {},
        {}
      )
      expect(result).toBe("SELECT * FROM measures WHERE name = 'cpu_usage' AND host = 'server1'")
    })

    it('should handle longer variable names first to avoid partial matches', () => {
      const sql = 'SELECT $metric, $metric_name'
      const result = substituteMacros(
        sql,
        { metric: 'cpu', metric_name: 'CPU Usage' },
        defaultTimeRange,
        {},
        {}
      )
      expect(result).toBe('SELECT cpu, CPU Usage')
    })

    it('should not substitute variable-like strings that are not word boundaries', () => {
      const sql = 'SELECT $metric_extended'
      const result = substituteMacros(sql, { metric: 'cpu' }, defaultTimeRange, {}, {})
      // Should NOT match because $metric is followed by _extended (not a word boundary)
      expect(result).toBe('SELECT $metric_extended')
    })

    it('should substitute at word boundaries', () => {
      const sql = 'SELECT $metric FROM table'
      const result = substituteMacros(sql, { metric: 'cpu' }, defaultTimeRange, {}, {})
      expect(result).toBe('SELECT cpu FROM table')
    })
  })

  describe('edge cases', () => {
    it('should leave unmatched variables unchanged', () => {
      const sql = 'SELECT $unknown'
      const result = substituteMacros(sql, {}, defaultTimeRange, {}, {})
      expect(result).toBe('SELECT $unknown')
    })

    it('should handle empty SQL', () => {
      const result = substituteMacros('', {}, defaultTimeRange)
      expect(result).toBe('')
    })

    it('should handle SQL with no variables', () => {
      const sql = 'SELECT * FROM logs LIMIT 100'
      const result = substituteMacros(sql, { metric: 'cpu' }, defaultTimeRange, {}, {})
      expect(result).toBe('SELECT * FROM logs LIMIT 100')
    })

    it('should handle empty variables object', () => {
      const sql = 'SELECT $metric'
      const result = substituteMacros(sql, {}, defaultTimeRange, {}, {})
      expect(result).toBe('SELECT $metric')
    })

    it('should handle variable with empty string value', () => {
      const sql = "SELECT * FROM logs WHERE filter = '$filter'"
      const result = substituteMacros(sql, { filter: '' }, defaultTimeRange, {}, {})
      expect(result).toBe("SELECT * FROM logs WHERE filter = ''")
    })
  })

  describe('multi-column variable substitution', () => {
    it('should substitute $variable.column with specific column value', () => {
      const sql = "SELECT * FROM measures WHERE name = '$metric.name'"
      const result = substituteMacros(
        sql,
        { metric: { name: 'cpu_usage', unit: 'percent' } },
        defaultTimeRange,
        {},
        {}
      )
      expect(result).toBe("SELECT * FROM measures WHERE name = 'cpu_usage'")
    })

    it('should substitute multiple column references', () => {
      const sql = "SELECT '$metric.name' AS name, '$metric.unit' AS unit"
      const result = substituteMacros(
        sql,
        { metric: { name: 'cpu_usage', unit: 'percent' } },
        defaultTimeRange,
        {},
        {}
      )
      expect(result).toBe("SELECT 'cpu_usage' AS name, 'percent' AS unit")
    })

    it('should leave unresolved column references unchanged', () => {
      const sql = "SELECT '$metric.unknown'"
      const result = substituteMacros(
        sql,
        { metric: { name: 'cpu_usage', unit: 'percent' } },
        defaultTimeRange,
        {},
        {}
      )
      expect(result).toBe("SELECT '$metric.unknown'")
    })

    it('should leave dotted references unchanged for simple string variables', () => {
      const sql = "SELECT '$metric.name'"
      const result = substituteMacros(
        sql,
        { metric: 'cpu_usage' },
        defaultTimeRange,
        {},
        {}
      )
      // Simple variable can't have column access, left unchanged
      expect(result).toBe("SELECT '$metric.name'")
    })

    it('should use JSON when accessing multi-column variable without column', () => {
      const sql = "SELECT '$metric'"
      const result = substituteMacros(
        sql,
        { metric: { name: 'cpu_usage', unit: 'percent' } },
        defaultTimeRange,
        {},
        {}
      )
      expect(result).toBe(`SELECT '{"name":"cpu_usage","unit":"percent"}'`)
    })

    it('should escape single quotes in column values', () => {
      const sql = "SELECT '$metric.description'"
      const result = substituteMacros(
        sql,
        { metric: { name: 'cpu', description: "it's hot" } },
        defaultTimeRange,
        {},
        {}
      )
      expect(result).toBe("SELECT 'it''s hot'")
    })

    it('should handle mixed simple and multi-column variables', () => {
      const sql = "SELECT * FROM $table WHERE name = '$metric.name' AND host = '$host'"
      const result = substituteMacros(
        sql,
        {
          table: 'measures',
          metric: { name: 'cpu', unit: 'percent' },
          host: 'server1',
        },
        defaultTimeRange,
        {},
        {}
      )
      expect(result).toBe("SELECT * FROM measures WHERE name = 'cpu' AND host = 'server1'")
    })
  })
})

describe('validateMacros', () => {
  it('should return valid for correct simple variable references', () => {
    const result = validateMacros('SELECT $metric', { metric: 'cpu' }, {}, {})
    expect(result.valid).toBe(true)
    expect(result.errors).toHaveLength(0)
  })

  it('should return valid for correct dotted variable references', () => {
    const result = validateMacros(
      "SELECT '$metric.name'",
      { metric: { name: 'cpu', unit: 'percent' } },
      {},
      {}
    )
    expect(result.valid).toBe(true)
    expect(result.errors).toHaveLength(0)
  })

  it('should report error for unknown variable', () => {
    const result = validateMacros('SELECT $unknown', {}, {}, {})
    expect(result.valid).toBe(false)
    expect(result.errors).toContain('Unknown variable: unknown')
  })

  it('should report error for dotted access on simple variable', () => {
    const result = validateMacros("SELECT '$metric.name'", { metric: 'cpu' }, {}, {})
    expect(result.valid).toBe(false)
    expect(result.errors[0]).toContain("Variable 'metric' is not a multi-column variable")
  })

  it('should report error for unknown column in multi-column variable', () => {
    const result = validateMacros(
      "SELECT '$metric.unknown'",
      { metric: { name: 'cpu', unit: 'percent' } },
      {},
      {}
    )
    expect(result.valid).toBe(false)
    expect(result.errors[0]).toContain("Column 'unknown' not found in variable 'metric'")
    expect(result.errors[0]).toContain('Available: name, unit')
  })

  it('should ignore built-in variables', () => {
    const result = validateMacros('SELECT * FROM logs WHERE time >= $from AND time <= $to', {}, {}, {})
    expect(result.valid).toBe(true)
  })

  it('should ignore $order_by special variable', () => {
    const result = validateMacros('SELECT * FROM logs ORDER BY $order_by', {}, {}, {})
    expect(result.valid).toBe(true)
  })

  it('should not report "Unknown variable" for valid cell result references', () => {
    const table = tableFromArrays({ process_id: ['abc123'] })
    const result = validateMacros(
      "SELECT * FROM view_instance('log_entries', '$game_session[0].process_id')",
      {},
      { game_session: table },
      {},
    )
    expect(result.valid).toBe(true)
    expect(result.errors).toHaveLength(0)
  })
})

describe('cell result ref substitution', () => {
  const defaultTimeRange = { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' }

  it('should substitute $cell[0].col from an Arrow Table', () => {
    const table = tableFromArrays({ process_id: ['abc123'], name: ['server1'] })
    const result = substituteMacros(
      "SELECT * FROM view_instance('log_entries', '$game_session[0].process_id')",
      {},
      defaultTimeRange,
      { game_session: table },
      {},
    )
    expect(result).toBe("SELECT * FROM view_instance('log_entries', 'abc123')")
  })

  it('should leave macro unresolved for out-of-bounds row index', () => {
    const table = tableFromArrays({ col: ['val'] })
    const result = substituteMacros(
      'SELECT $cell[5].col',
      {},
      defaultTimeRange,
      { cell: table },
      {},
    )
    expect(result).toBe('SELECT $cell[5].col')
  })

  it('should leave macro unresolved for missing column', () => {
    const table = tableFromArrays({ col: ['val'] })
    const result = substituteMacros(
      'SELECT $cell[0].missing',
      {},
      defaultTimeRange,
      { cell: table },
      {},
    )
    expect(result).toBe('SELECT $cell[0].missing')
  })

  it('should leave macro unresolved for unknown cell', () => {
    const result = substituteMacros(
      'SELECT $unknown_cell[0].col',
      {},
      defaultTimeRange,
      {},
      {},
    )
    expect(result).toBe('SELECT $unknown_cell[0].col')
  })

  it('should not interfere with existing $variable.column patterns', () => {
    const table = tableFromArrays({ id: ['1'] })
    const result = substituteMacros(
      "SELECT '$metric.name', '$cell[0].id'",
      { metric: { name: 'cpu', unit: 'pct' } },
      defaultTimeRange,
      { cell: table },
      {},
    )
    expect(result).toBe("SELECT 'cpu', '1'")
  })

  it('should not interfere with simple $variable patterns', () => {
    const table = tableFromArrays({ id: ['1'] })
    const result = substituteMacros(
      "SELECT '$host', '$cell[0].id'",
      { host: 'server1' },
      defaultTimeRange,
      { cell: table },
      {},
    )
    expect(result).toBe("SELECT 'server1', '1'")
  })

  it('should escape single quotes in cell result values', () => {
    const table = tableFromArrays({ msg: ["it's working"] })
    const result = substituteMacros(
      "SELECT '$cell[0].msg'",
      {},
      defaultTimeRange,
      { cell: table },
      {},
    )
    expect(result).toBe("SELECT 'it''s working'")
  })

  it('simple variable pattern should not partially match $cell_name in $cell_name[0].col', () => {
    const table = tableFromArrays({ id: ['abc'] })
    const result = substituteMacros(
      'SELECT $cell_name[0].id',
      { cell_name: 'should_not_match' },
      defaultTimeRange,
      { cell_name: table },
      {},
    )
    expect(result).toBe('SELECT abc')
  })

  it('should format timestamp values as RFC3339', () => {
    // Build a table with a Timestamp(MILLISECOND) column
    const timestampType = new Timestamp(TimeUnit.MILLISECOND, null)
    // 2024-01-15T10:30:00.000Z in milliseconds
    const ms = 1705314600000
    const vector = vectorFromArray([ms], timestampType)
    const table = new Table({ start_time: vector })

    const result = substituteMacros(
      "SELECT '$cell[0].start_time'",
      {},
      defaultTimeRange,
      { cell: table },
      {},
    )
    expect(result).toBe("SELECT '2024-01-15T10:30:00.000Z'")
  })

  it('should still format non-time columns as plain strings', () => {
    const table = tableFromArrays({ name: ['server1'] })
    const result = substituteMacros(
      "SELECT '$cell[0].name'",
      {},
      defaultTimeRange,
      { cell: table },
      {},
    )
    expect(result).toBe("SELECT 'server1'")
  })
})

describe('validateMacros with cell results', () => {
  it('should report error for unknown cell name', () => {
    const result = validateMacros('$unknown[0].col', {}, { }, {})
    expect(result.valid).toBe(false)
    expect(result.errors).toContain('Unknown cell: unknown')
  })

  it('should report error for out-of-bounds row index', () => {
    const table = tableFromArrays({ col: ['val'] })
    const result = validateMacros('$cell[5].col', {}, { cell: table }, {})
    expect(result.valid).toBe(false)
    expect(result.errors[0]).toContain('Row index 5 out of bounds')
    expect(result.errors[0]).toContain('1 rows')
  })

  it('should report error for unknown column', () => {
    const table = tableFromArrays({ col: ['val'], other: ['x'] })
    const result = validateMacros('$cell[0].missing', {}, { cell: table }, {})
    expect(result.valid).toBe(false)
    expect(result.errors[0]).toContain("Column 'missing' not found in cell 'cell'")
    expect(result.errors[0]).toContain('col, other')
  })

  it('should pass for valid cell result reference', () => {
    const table = tableFromArrays({ col: ['val'] })
    const result = validateMacros('$cell[0].col', {}, { cell: table }, {})
    expect(result.valid).toBe(true)
  })

  it('should report unknown cell when cellResults is empty', () => {
    const result = validateMacros('$cell[0].col', {}, {}, {})
    // With an empty cellResults, the cell reference is unknown
    expect(result.valid).toBe(false)
    expect(result.errors).toContain('Unknown cell: cell')
  })
})

describe('VariableValue helpers', () => {
  describe('serializeVariableValue', () => {
    it('should return string as-is', () => {
      expect(serializeVariableValue('cpu_usage')).toBe('cpu_usage')
    })

    it('should prefix and JSON-encode object', () => {
      const result = serializeVariableValue({ name: 'cpu', unit: 'percent' })
      expect(result).toBe('mcol:{"name":"cpu","unit":"percent"}')
    })

    it('should handle empty object', () => {
      expect(serializeVariableValue({})).toBe('mcol:{}')
    })

    it('should not add prefix to strings starting with curly brace', () => {
      expect(serializeVariableValue('{literal}')).toBe('{literal}')
    })
  })

  describe('deserializeVariableValue', () => {
    it('should return simple string as-is', () => {
      expect(deserializeVariableValue('cpu_usage')).toBe('cpu_usage')
    })

    it('should parse prefixed JSON object', () => {
      const result = deserializeVariableValue('mcol:{"name":"cpu","unit":"percent"}')
      expect(result).toEqual({ name: 'cpu', unit: 'percent' })
    })

    it('should return string if prefix present but JSON is invalid', () => {
      expect(deserializeVariableValue('mcol:{invalid')).toBe('mcol:{invalid')
    })

    it('should return string without prefix as-is (no magic parsing)', () => {
      // Without mcol: prefix, curly braces are treated as literal string
      expect(deserializeVariableValue('{"name":"cpu"}')).toBe('{"name":"cpu"}')
    })

    it('should return string if prefixed JSON is array', () => {
      expect(deserializeVariableValue('mcol:["a","b"]')).toBe('mcol:["a","b"]')
    })

    it('should return string if prefixed JSON object has non-string values', () => {
      expect(deserializeVariableValue('mcol:{"a":123}')).toBe('mcol:{"a":123}')
    })

    it('should handle prefixed empty object', () => {
      expect(deserializeVariableValue('mcol:{}')).toEqual({})
    })
  })

  describe('getVariableString', () => {
    it('should return string as-is', () => {
      expect(getVariableString('cpu_usage')).toBe('cpu_usage')
    })

    it('should return JSON for object', () => {
      expect(getVariableString({ name: 'cpu', unit: 'percent' })).toBe('{"name":"cpu","unit":"percent"}')
    })

    it('should return empty object JSON for empty object', () => {
      expect(getVariableString({})).toBe('{}')
    })
  })

  describe('isMultiColumnValue', () => {
    it('should return false for string', () => {
      expect(isMultiColumnValue('cpu_usage')).toBe(false)
    })

    it('should return true for object', () => {
      expect(isMultiColumnValue({ name: 'cpu' })).toBe(true)
    })

    it('should return true for empty object', () => {
      expect(isMultiColumnValue({})).toBe(true)
    })
  })
})

describe('sanitizeCellName', () => {
  it('should convert spaces to underscores', () => {
    expect(sanitizeCellName('My Cell')).toBe('My_Cell')
    expect(sanitizeCellName('Table 2')).toBe('Table_2')
    // Multiple spaces become a single underscore
    expect(sanitizeCellName('Multi  Space')).toBe('Multi_Space')
  })

  it('should remove non-ASCII characters', () => {
    expect(sanitizeCellName('Test\u00e9')).toBe('Test')
    expect(sanitizeCellName('\u4e2d\u6587Name')).toBe('Name')
    expect(sanitizeCellName('Caf\u00e9_Name')).toBe('Caf_Name')
  })

  it('should remove special characters', () => {
    expect(sanitizeCellName('Test-Name')).toBe('TestName')
    expect(sanitizeCellName('Test.Name')).toBe('TestName')
    expect(sanitizeCellName('Test@Name!')).toBe('TestName')
  })

  it('should prefix with underscore if starts with number', () => {
    expect(sanitizeCellName('123Test')).toBe('_123Test')
    expect(sanitizeCellName('1')).toBe('_1')
  })

  it('should preserve valid identifiers', () => {
    expect(sanitizeCellName('ValidName')).toBe('ValidName')
    expect(sanitizeCellName('valid_name_2')).toBe('valid_name_2')
    expect(sanitizeCellName('_private')).toBe('_private')
  })
})

describe('validateCellName', () => {
  it('should return error for empty name', () => {
    expect(validateCellName('', new Set())).toBe('Cell name cannot be empty')
    expect(validateCellName('   ', new Set())).toBe('Cell name cannot be empty')
  })

  it('should return error for non-ASCII characters', () => {
    expect(validateCellName('Caf\u00e9', new Set())).toBe('Cell name can only contain ASCII characters')
    expect(validateCellName('\u4e2d\u6587', new Set())).toBe('Cell name can only contain ASCII characters')
  })

  it('should return error for invalid characters', () => {
    expect(validateCellName('Test-Name', new Set())).toBe('Cell name can only contain letters, numbers, underscores, and spaces')
    expect(validateCellName('Test@Name', new Set())).toBe('Cell name can only contain letters, numbers, underscores, and spaces')
  })

  it('should return error for duplicate names after sanitization', () => {
    const existingNames = new Set(['Table_2'])
    expect(validateCellName('Table 2', existingNames)).toBe('A cell with this name already exists')
  })

  it('should allow same name for current cell', () => {
    const existingNames = new Set(['Table_2'])
    expect(validateCellName('Table 2', existingNames, 'Table_2')).toBeNull()
  })

  it('should return null for valid names', () => {
    expect(validateCellName('ValidName', new Set())).toBeNull()
    expect(validateCellName('Valid Name', new Set())).toBeNull()
    expect(validateCellName('Valid_Name_2', new Set())).toBeNull()
  })
})

describe('createDefaultCell', () => {
  describe('name generation', () => {
    it('should create cell with capitalized type name', () => {
      const cell = createDefaultCell('table', new Set())
      expect(cell.name).toBe('Table')
    })

    it('should create unique name when base name exists', () => {
      const existingNames = new Set(['Table'])
      const cell = createDefaultCell('table', existingNames)
      expect(cell.name).toBe('Table_2')
    })

    it('should create unique name with incrementing counter', () => {
      const existingNames = new Set(['Table', 'Table_2', 'Table_3'])
      const cell = createDefaultCell('table', existingNames)
      expect(cell.name).toBe('Table_4')
    })

    it('should generate correct names for all cell types', () => {
      expect(createDefaultCell('table', new Set()).name).toBe('Table')
      expect(createDefaultCell('chart', new Set()).name).toBe('Chart')
      expect(createDefaultCell('log', new Set()).name).toBe('Log')
      expect(createDefaultCell('markdown', new Set()).name).toBe('Markdown')
      expect(createDefaultCell('variable', new Set()).name).toBe('Variable')
    })
  })

  describe('table cell', () => {
    it('should create table cell with correct type', () => {
      const cell = createDefaultCell('table', new Set())
      expect(cell.type).toBe('table')
    })

    it('should include default SQL', () => {
      const cell = createDefaultCell('table', new Set())
      expect(cell).toHaveProperty('sql')
      expect((cell as { sql: string }).sql).toBe(DEFAULT_SQL.table)
    })

    it('should have default fixed height layout', () => {
      const cell = createDefaultCell('table', new Set())
      expect(cell.layout).toEqual({ height: 300 })
    })
  })

  describe('chart cell', () => {
    it('should create chart cell with correct type and SQL', () => {
      const cell = createDefaultCell('chart', new Set())
      expect(cell.type).toBe('chart')
      expect((cell as { sql: string }).sql).toBe(DEFAULT_SQL.chart)
    })
  })

  describe('log cell', () => {
    it('should create log cell with correct type and SQL', () => {
      const cell = createDefaultCell('log', new Set())
      expect(cell.type).toBe('log')
      expect((cell as { sql: string }).sql).toBe(DEFAULT_SQL.log)
    })
  })

  describe('markdown cell', () => {
    it('should create markdown cell with correct type', () => {
      const cell = createDefaultCell('markdown', new Set())
      expect(cell.type).toBe('markdown')
    })

    it('should include default content', () => {
      const cell = createDefaultCell('markdown', new Set())
      expect(cell).toHaveProperty('content')
      expect((cell as { content: string }).content).toContain('# Notes')
    })

    it('should not have sql property', () => {
      const cell = createDefaultCell('markdown', new Set())
      expect(cell).not.toHaveProperty('sql')
    })
  })

  describe('variable cell', () => {
    it('should create variable cell with correct type', () => {
      const cell = createDefaultCell('variable', new Set())
      expect(cell.type).toBe('variable')
    })

    it('should default to combobox variable type', () => {
      const cell = createDefaultCell('variable', new Set())
      expect((cell as { variableType: string }).variableType).toBe('combobox')
    })

    it('should include default SQL for options', () => {
      const cell = createDefaultCell('variable', new Set())
      expect((cell as { sql: string }).sql).toBe(DEFAULT_SQL.variable)
    })
  })

  // Note: unknown type fallback removed - with the new metadata-based design,
  // TypeScript enforces valid cell types and the registry would throw for unknown types
})

describe('selected row ref substitution', () => {
  const defaultTimeRange = { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' }

  it('should substitute $cell.selected.col from a selection object', () => {
    const result = substituteMacros(
      "SELECT * FROM view_instance('log_entries', '$processes.selected.process_id')",
      {},
      defaultTimeRange,
      {},
      { processes: { process_id: 'abc123', exe: 'server1' } },
    )
    expect(result).toBe("SELECT * FROM view_instance('log_entries', 'abc123')")
  })

  it('should resolve to empty string for missing cell', () => {
    const result = substituteMacros(
      'SELECT $unknown.selected.col',
      {},
      defaultTimeRange,
      {},
      {},
    )
    expect(result).toBe('SELECT ')
  })

  it('should resolve to empty string for missing column in selection', () => {
    const result = substituteMacros(
      'SELECT $cell.selected.missing',
      {},
      defaultTimeRange,
      {},
      { cell: { col: 'val' } },
    )
    expect(result).toBe('SELECT ')
  })

  it('should resolve to empty string when no selection exists (cell not in cellSelections)', () => {
    const result = substituteMacros(
      'SELECT $cell.selected.col',
      {},
      defaultTimeRange,
      {},
      {},
    )
    expect(result).toBe('SELECT ')
  })

  it('should escape single quotes in selected values', () => {
    const result = substituteMacros(
      "SELECT '$cell.selected.msg'",
      {},
      defaultTimeRange,
      {},
      { cell: { msg: "it's working" } },
    )
    expect(result).toBe("SELECT 'it''s working'")
  })

  it('should not interfere with $cell[N].col pattern', () => {
    const table = tableFromArrays({ id: ['row0'] })
    const result = substituteMacros(
      "SELECT '$cell[0].id', '$cell.selected.id'",
      {},
      defaultTimeRange,
      { cell: table },
      { cell: { id: 'selected_val' } },
    )
    expect(result).toBe("SELECT 'row0', 'selected_val'")
  })

  it('should not interfere with $variable.column pattern', () => {
    const result = substituteMacros(
      "SELECT '$metric.name', '$cell.selected.id'",
      { metric: { name: 'cpu', unit: 'pct' } },
      defaultTimeRange,
      {},
      { cell: { id: 'abc' } },
    )
    expect(result).toBe("SELECT 'cpu', 'abc'")
  })

  it('should format timestamp values from selection using Arrow table schema', () => {
    const timestampType = new Timestamp(TimeUnit.MILLISECOND, null)
    const ms = 1705314600000
    const vector = vectorFromArray([ms], timestampType)
    const table = new Table({ start_time: vector })

    const result = substituteMacros(
      "SELECT '$cell.selected.start_time'",
      {},
      defaultTimeRange,
      { cell: table },
      { cell: { start_time: ms } },
    )
    expect(result).toBe("SELECT '2024-01-15T10:30:00.000Z'")
  })

  it('should handle null values in selection gracefully', () => {
    const result = substituteMacros(
      'SELECT $cell.selected.col',
      {},
      defaultTimeRange,
      {},
      { cell: { col: null as unknown } },
    )
    expect(result).toBe('SELECT ')
  })
})

describe('validateMacros with cell selections', () => {
  it('should report error for unknown cell in selection reference', () => {
    const result = validateMacros(
      '$unknown.selected.col',
      {},
      {},
      { some_cell: { col: 'val' } },
    )
    expect(result.valid).toBe(false)
    expect(result.errors).toContain('Unknown cell: unknown')
  })

  it('should report error for unknown column in selection reference', () => {
    const result = validateMacros(
      '$cell.selected.missing',
      {},
      {},
      { cell: { col: 'val', other: 'x' } },
    )
    expect(result.valid).toBe(false)
    expect(result.errors[0]).toContain("Column 'missing' not found in cell 'cell'")
    expect(result.errors[0]).toContain('col, other')
  })

  it('should pass for valid selection reference', () => {
    const result = validateMacros(
      '$cell.selected.col',
      {},
      {},
      { cell: { col: 'val' } },
    )
    expect(result.valid).toBe(true)
  })

  it('should report unknown cell when cellSelections is empty', () => {
    const result = validateMacros('$cell.selected.col', {}, {}, {})
    expect(result.valid).toBe(false)
    expect(result.errors).toContain('Unknown cell: cell')
  })

  it('should not report $cell.selected as unknown variable in dotted validation', () => {
    const result = validateMacros(
      '$cell.selected.col',
      {},
      {},
      { cell: { col: 'val' } },
    )
    expect(result.valid).toBe(true)
    expect(result.errors).toHaveLength(0)
  })
})

describe('evaluateTemplate', () => {
  const defaultTimeRange = { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' }
  const emptyCtx = () => ({
    variables: {},
    timeRange: defaultTimeRange,
    cellResults: {},
    cellSelections: {},
  })

  describe('format_value calls', () => {
    it('formats a numeric literal with the bytes unit', () => {
      const { text, warnings } = evaluateTemplate("format_value(3678630912, 'bytes')", emptyCtx())
      expect(text).toBe('3.4 GB')
      expect(warnings).toEqual([])
    })

    it('formats a multi-column variable value with its unit column', () => {
      const ctx = {
        ...emptyCtx(),
        variables: { metric_avg: '3678630912', metric: { name: 'mem', unit: 'bytes' } },
      }
      const { text, warnings } = evaluateTemplate('format_value($metric_avg, $metric.unit)', ctx)
      expect(text).toBe('3.4 GB')
      expect(warnings).toEqual([])
    })

    it('formats a selected cell value', () => {
      const ctx = {
        ...emptyCtx(),
        cellSelections: { stat: { bytes: 3678630912 } },
      }
      const { text, warnings } = evaluateTemplate("format_value($cell.selected.bytes, 'bytes')", { ...ctx, cellSelections: { cell: { bytes: 3678630912 } } })
      expect(text).toBe('3.4 GB')
      expect(warnings).toEqual([])
    })

    it('formats a BigInt cell value (timestamp-like arg)', () => {
      const table = tableFromArrays({ duration_ns: [BigInt(2_500_000_000)] })
      const ctx = {
        ...emptyCtx(),
        cellResults: { stats: table },
      }
      const { text, warnings } = evaluateTemplate("format_value($stats[0].duration_ns, 'nanoseconds')", ctx)
      expect(text).toBe('2.50 seconds')
      expect(warnings).toEqual([])
    })

    it('rejects a Timestamp column passed to format_value (precision-loss guard)', () => {
      // Wall-clock timestamps would otherwise coerce via Number(BigInt(~1.7e18))
      // to a finite-but-garbage value and format_value would render nonsense
      // like "53954068.94 years". The fix routes time-typed args through
      // formatArrowValue (ISO string) so format_value sees NaN and warns.
      const tsType = new Timestamp(TimeUnit.MILLISECOND, null)
      const vector = vectorFromArray([1705314600000], tsType)
      const table = new Table({ ts: vector })
      const ctx = { ...emptyCtx(), cellResults: { events: table } }
      const { text, warnings } = evaluateTemplate(
        "format_value($events[0].ts, 'milliseconds')",
        ctx,
      )
      expect(text).toBe("format_value($events[0].ts, 'milliseconds')")
      expect(warnings).toEqual(['format_value: invalid argument value'])
    })

    it('flags an unknown function name when args use template syntax', () => {
      const ctx = {
        ...emptyCtx(),
        variables: { x: 'X' },
      }
      const { text, warnings } = evaluateTemplate('random_word($x)', ctx)
      expect(text).toBe('random_word($x)')
      expect(warnings).toEqual(['Unknown template function: random_word'])
    })

    it('does NOT flag function-like prose that uses no template macros', () => {
      // Markdown commonly contains prose like `Math.max(1, 2)` — flagging
      // it would be noisy. The heuristic is: warn only when at least one
      // arg is a $-macro.
      const { text, warnings } = evaluateTemplate('See Math.max(1, 2) and foo(3.14, "bar").', emptyCtx())
      expect(text).toBe('See Math.max(1, 2) and foo(3.14, "bar").')
      expect(warnings).toEqual([])
    })

    it('emits the original call source plus a warning when an arg macro is unresolved', () => {
      const { text, warnings } = evaluateTemplate("format_value($missing, 'bytes')", emptyCtx())
      expect(text).toBe("format_value($missing, 'bytes')")
      expect(warnings).toEqual(['format_value: $missing is unresolved'])
    })

    it('preserves the call source when the selection arg is unresolved (no half-substituted state)', () => {
      const { text, warnings } = evaluateTemplate("format_value($cell.selected.bytes, 'bytes')", emptyCtx())
      expect(text).toBe("format_value($cell.selected.bytes, 'bytes')")
      expect(warnings).toEqual(['format_value: $cell.selected.bytes is unresolved'])
    })

    it('accepts string literals containing commas', () => {
      const ctx = { ...emptyCtx(), variables: { x: '12' } }
      const { text } = evaluateTemplate("format_value($x, 'GB, please')", ctx)
      // Unknown unit -> falls back to "{value.toLocaleString()} {unit}"
      expect(text).toBe('12 GB, please')
    })

    it('substitutes a function call and a naked variable in the same template', () => {
      const ctx = {
        ...emptyCtx(),
        variables: { x: '2048', y: 'YVAL' },
      }
      const { text } = evaluateTemplate("format_value($x, 'bytes') extra $y", ctx)
      expect(text).toBe('2.0 KB extra YVAL')
    })

    it('deduplicates the same unresolved arg referenced in multiple calls', () => {
      const { warnings } = evaluateTemplate(
        "format_value($missing, 'bytes') and format_value($missing, 'seconds')",
        emptyCtx(),
      )
      expect(warnings).toEqual(['format_value: $missing is unresolved'])
    })
  })

  describe('macro behavior', () => {
    it('substitutes a naked variable like substituteMacrosRaw would', () => {
      const ctx = { ...emptyCtx(), variables: { variable: 'X' } }
      const { text, warnings } = evaluateTemplate('a $variable b', ctx)
      expect(text).toBe('a X b')
      expect(warnings).toEqual([])
    })

    it('leaves a naked unresolved $cell.selected.col in place AND emits a warning (regression-pin §6 #2)', () => {
      const { text, warnings } = evaluateTemplate('a $cell.selected.col b', emptyCtx())
      expect(text).toBe('a $cell.selected.col b')
      expect(warnings).toEqual(['$cell.selected.col is unresolved'])
    })

    it('preserves single quotes verbatim (quote-escape regression §6 #1)', () => {
      const ctx = { ...emptyCtx(), variables: { search: "it's working" } }
      const { text } = evaluateTemplate('msg: $search', ctx)
      expect(text).toBe("msg: it's working")
    })
  })

  describe('bareColumnsFromRow (Map detail templates)', () => {
    it('renders a bare $col Timestamp as RFC3339 via its column DataType', () => {
      const tsType = new Timestamp(TimeUnit.MILLISECOND, null)
      const ctx = {
        ...emptyCtx(),
        row: { ts: 1705314600000 },
        columnTypes: new Map([['ts', tsType]]),
        bareColumnsFromRow: true,
      }
      const { text, warnings } = evaluateTemplate('At $ts', ctx)
      expect(text).toBe('At 2024-01-15T10:30:00.000Z')
      expect(warnings).toEqual([])
    })

    it('feeds format_value the raw BigInt column value (precision preserved)', () => {
      const ctx = {
        ...emptyCtx(),
        row: { size: BigInt(3678630912) },
        bareColumnsFromRow: true,
      }
      const { text, warnings } = evaluateTemplate("format_value($size, 'bytes')", ctx)
      expect(text).toBe('3.4 GB')
      expect(warnings).toEqual([])
    })

    it('prefers the row column over a same-named variable', () => {
      const ctx = {
        ...emptyCtx(),
        variables: { shared: 'from-var' },
        row: { shared: 'from-row' },
        bareColumnsFromRow: true,
      }
      const { text } = evaluateTemplate('value=$shared', ctx)
      expect(text).toBe('value=from-row')
    })

    it('falls back to the variable when the row lacks the column', () => {
      const ctx = {
        ...emptyCtx(),
        variables: { only_var: 'V' },
        row: { other: 'O' },
        bareColumnsFromRow: true,
      }
      const { text } = evaluateTemplate('value=$only_var', ctx)
      expect(text).toBe('value=V')
    })

    it('with the flag off, a bare $name resolves the variable, not the row (table-override default)', () => {
      const ctx = {
        ...emptyCtx(),
        variables: { shared: 'from-var' },
        row: { shared: 'from-row' },
      }
      const { text } = evaluateTemplate('value=$shared', ctx)
      expect(text).toBe('value=from-var')
    })
  })
})

// Pins the shared lookup layer directly, independent of either engine's
// formatting/escaping. resolveMacro returns raw values + a resolved flag; the
// '' / leave-source / warning mappings are the callers' job (covered above).
describe('resolveMacro', () => {
  const emptyCtx = (): ResolveCtx => ({
    variables: {},
    cellResults: {},
    cellSelections: {},
  })

  describe('time', () => {
    const timeRange = { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' }

    it('resolves $from / $to when timeRange is set', () => {
      const ctx = { ...emptyCtx(), timeRange }
      expect(resolveMacro({ kind: 'time', which: 'from' }, ctx)).toEqual({ value: timeRange.begin, resolved: true })
      expect(resolveMacro({ kind: 'time', which: 'to' }, ctx)).toEqual({ value: timeRange.end, resolved: true })
    })

    it('is unresolved when timeRange is omitted', () => {
      expect(resolveMacro({ kind: 'time', which: 'from' }, emptyCtx())).toEqual({ value: undefined, resolved: false })
    })
  })

  describe('cellRow', () => {
    it('resolves with value and column dataType', () => {
      const tsType = new Timestamp(TimeUnit.MILLISECOND, null)
      const table = new Table({
        name: vectorFromArray(['server1']),
        ts: vectorFromArray([1705314600000], tsType),
      })
      const ctx = { ...emptyCtx(), cellResults: { cell: table } }

      const r = resolveMacro({ kind: 'cellRow', cell: 'cell', rowIdx: 0, col: 'name' }, ctx)
      expect(r.resolved).toBe(true)
      expect(r.value).toBe('server1')

      const ts = resolveMacro({ kind: 'cellRow', cell: 'cell', rowIdx: 0, col: 'ts' }, ctx)
      expect(ts.resolved).toBe(true)
      expect(ts.dataType).toBe(tsType)
    })

    it('is unresolved on missing table, OOB row, or null cell', () => {
      const table = tableFromArrays({ col: ['val'] })
      const ctx = { ...emptyCtx(), cellResults: { cell: table } }
      expect(resolveMacro({ kind: 'cellRow', cell: 'nope', rowIdx: 0, col: 'col' }, ctx).resolved).toBe(false)
      expect(resolveMacro({ kind: 'cellRow', cell: 'cell', rowIdx: 5, col: 'col' }, ctx).resolved).toBe(false)
      expect(resolveMacro({ kind: 'cellRow', cell: 'cell', rowIdx: 0, col: 'missing' }, ctx).resolved).toBe(false)
    })
  })

  describe('selected', () => {
    it('resolves with dataType taken from cellResults', () => {
      const tsType = new Timestamp(TimeUnit.MILLISECOND, null)
      const table = new Table({ ts: vectorFromArray([1705314600000], tsType) })
      const ctx = {
        ...emptyCtx(),
        cellResults: { cell: table },
        cellSelections: { cell: { ts: 1705314600000, name: 'srv' } },
      }
      const r = resolveMacro({ kind: 'selected', cell: 'cell', col: 'ts' }, ctx)
      expect(r.resolved).toBe(true)
      expect(r.value).toBe(1705314600000)
      expect(r.dataType).toBe(tsType)
    })

    it('is unresolved on missing selection or null value', () => {
      const ctx = { ...emptyCtx(), cellSelections: { cell: { col: null } } }
      expect(resolveMacro({ kind: 'selected', cell: 'nope', col: 'col' }, ctx).resolved).toBe(false)
      expect(resolveMacro({ kind: 'selected', cell: 'cell', col: 'col' }, ctx).resolved).toBe(false)
    })
  })

  describe('rowCol', () => {
    it('resolves only when ctx.row is set, with dataType from columnTypes', () => {
      const tsType = new Timestamp(TimeUnit.MILLISECOND, null)
      const ctx = {
        ...emptyCtx(),
        row: { ts: 1705314600000, name: 'srv' },
        columnTypes: new Map([['ts', tsType]]),
      }
      const r = resolveMacro({ kind: 'rowCol', col: 'ts' }, ctx)
      expect(r).toEqual({ value: 1705314600000, resolved: true, dataType: tsType })
      expect(resolveMacro({ kind: 'rowCol', col: 'name' }, ctx).dataType).toBeUndefined()
    })

    it('is unresolved with no row, or a null / missing column', () => {
      expect(resolveMacro({ kind: 'rowCol', col: 'x' }, emptyCtx()).resolved).toBe(false)
      const ctx = { ...emptyCtx(), row: { x: null } }
      expect(resolveMacro({ kind: 'rowCol', col: 'x' }, ctx).resolved).toBe(false)
      expect(resolveMacro({ kind: 'rowCol', col: 'missing' }, ctx).resolved).toBe(false)
    })
  })

  describe('varCol', () => {
    it('resolves a column of a multi-column variable', () => {
      const ctx = { ...emptyCtx(), variables: { srv: { host: 'h1', port: '8080' } } }
      expect(resolveMacro({ kind: 'varCol', name: 'srv', col: 'host' }, ctx)).toEqual({ value: 'h1', resolved: true })
    })

    it('is unresolved for a string var, missing var, or missing column', () => {
      const ctx = { ...emptyCtx(), variables: { simple: 'v', srv: { host: 'h1' } } }
      expect(resolveMacro({ kind: 'varCol', name: 'simple', col: 'host' }, ctx).resolved).toBe(false)
      expect(resolveMacro({ kind: 'varCol', name: 'nope', col: 'host' }, ctx).resolved).toBe(false)
      expect(resolveMacro({ kind: 'varCol', name: 'srv', col: 'missing' }, ctx).resolved).toBe(false)
    })
  })

  describe('var', () => {
    it('resolves a simple string variable', () => {
      const ctx = { ...emptyCtx(), variables: { name: 'srv1' } }
      expect(resolveMacro({ kind: 'var', name: 'name' }, ctx)).toEqual({ value: 'srv1', resolved: true })
    })

    it('resolves a multi-column variable via getVariableString', () => {
      const value = { host: 'h1', port: '8080' }
      const ctx = { ...emptyCtx(), variables: { srv: value } }
      const r = resolveMacro({ kind: 'var', name: 'srv' }, ctx)
      expect(r.resolved).toBe(true)
      expect(r.value).toBe(getVariableString(value))
    })

    it('lets a row column win over a same-named variable when bareColumnsFromRow is set', () => {
      const ctx = {
        ...emptyCtx(),
        variables: { shared: 'from-var' },
        row: { shared: 'from-row' },
        bareColumnsFromRow: true,
      }
      expect(resolveMacro({ kind: 'var', name: 'shared' }, ctx).value).toBe('from-row')
    })

    it('falls back to the variable when bareColumnsFromRow row lookup is null', () => {
      const ctx = {
        ...emptyCtx(),
        variables: { shared: 'from-var' },
        row: { shared: null },
        bareColumnsFromRow: true,
      }
      expect(resolveMacro({ kind: 'var', name: 'shared' }, ctx).value).toBe('from-var')
    })

    it('is unresolved for an unknown name', () => {
      expect(resolveMacro({ kind: 'var', name: 'nope' }, emptyCtx()).resolved).toBe(false)
    })
  })
})
