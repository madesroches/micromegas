import { substituteMacros, createDefaultCell, DEFAULT_SQL } from '../notebook-utils'

describe('substituteMacros', () => {
  const defaultTimeRange = { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' }

  describe('time range substitution', () => {
    it('should substitute $begin with quoted timestamp', () => {
      const sql = 'SELECT * FROM logs WHERE time >= $begin'
      const result = substituteMacros(sql, {}, defaultTimeRange)
      expect(result).toBe("SELECT * FROM logs WHERE time >= '2024-01-01T00:00:00Z'")
    })

    it('should substitute $end with quoted timestamp', () => {
      const sql = 'SELECT * FROM logs WHERE time <= $end'
      const result = substituteMacros(sql, {}, defaultTimeRange)
      expect(result).toBe("SELECT * FROM logs WHERE time <= '2024-01-02T00:00:00Z'")
    })

    it('should substitute both $begin and $end', () => {
      const sql = 'SELECT * FROM logs WHERE time BETWEEN $begin AND $end'
      const result = substituteMacros(sql, {}, defaultTimeRange)
      expect(result).toBe(
        "SELECT * FROM logs WHERE time BETWEEN '2024-01-01T00:00:00Z' AND '2024-01-02T00:00:00Z'"
      )
    })

    it('should substitute multiple occurrences of $begin', () => {
      const sql = 'SELECT $begin, $begin'
      const result = substituteMacros(sql, {}, defaultTimeRange)
      expect(result).toBe("SELECT '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z'")
    })
  })

  describe('user variable substitution', () => {
    it('should substitute user variables without adding quotes', () => {
      const sql = "SELECT * FROM measures WHERE name = '$metric'"
      const result = substituteMacros(sql, { metric: 'cpu_usage' }, defaultTimeRange)
      expect(result).toBe("SELECT * FROM measures WHERE name = 'cpu_usage'")
    })

    it('should substitute variables without quotes when not wrapped', () => {
      const sql = 'SELECT $limit'
      const result = substituteMacros(sql, { limit: '100' }, defaultTimeRange)
      expect(result).toBe('SELECT 100')
    })

    it('should escape single quotes in variable values', () => {
      const sql = "SELECT * FROM logs WHERE msg = '$search'"
      const result = substituteMacros(sql, { search: "it's working" }, defaultTimeRange)
      expect(result).toBe("SELECT * FROM logs WHERE msg = 'it''s working'")
    })

    it('should escape multiple single quotes in variable values', () => {
      const sql = "SELECT * FROM logs WHERE msg = '$search'"
      const result = substituteMacros(sql, { search: "it's not it's" }, defaultTimeRange)
      expect(result).toBe("SELECT * FROM logs WHERE msg = 'it''s not it''s'")
    })

    it('should handle multiple different variables', () => {
      const sql = "SELECT * FROM measures WHERE name = '$metric' AND host = '$host'"
      const result = substituteMacros(
        sql,
        { metric: 'cpu_usage', host: 'server1' },
        defaultTimeRange
      )
      expect(result).toBe("SELECT * FROM measures WHERE name = 'cpu_usage' AND host = 'server1'")
    })

    it('should handle longer variable names first to avoid partial matches', () => {
      const sql = 'SELECT $metric, $metric_name'
      const result = substituteMacros(
        sql,
        { metric: 'cpu', metric_name: 'CPU Usage' },
        defaultTimeRange
      )
      expect(result).toBe('SELECT cpu, CPU Usage')
    })

    it('should not substitute variable-like strings that are not word boundaries', () => {
      const sql = 'SELECT $metric_extended'
      const result = substituteMacros(sql, { metric: 'cpu' }, defaultTimeRange)
      // Should NOT match because $metric is followed by _extended (not a word boundary)
      expect(result).toBe('SELECT $metric_extended')
    })

    it('should substitute at word boundaries', () => {
      const sql = 'SELECT $metric FROM table'
      const result = substituteMacros(sql, { metric: 'cpu' }, defaultTimeRange)
      expect(result).toBe('SELECT cpu FROM table')
    })
  })

  describe('edge cases', () => {
    it('should leave unmatched variables unchanged', () => {
      const sql = 'SELECT $unknown'
      const result = substituteMacros(sql, {}, defaultTimeRange)
      expect(result).toBe('SELECT $unknown')
    })

    it('should handle empty SQL', () => {
      const result = substituteMacros('', {}, defaultTimeRange)
      expect(result).toBe('')
    })

    it('should handle SQL with no variables', () => {
      const sql = 'SELECT * FROM logs LIMIT 100'
      const result = substituteMacros(sql, { metric: 'cpu' }, defaultTimeRange)
      expect(result).toBe('SELECT * FROM logs LIMIT 100')
    })

    it('should handle empty variables object', () => {
      const sql = 'SELECT $metric'
      const result = substituteMacros(sql, {}, defaultTimeRange)
      expect(result).toBe('SELECT $metric')
    })

    it('should handle variable with empty string value', () => {
      const sql = "SELECT * FROM logs WHERE filter = '$filter'"
      const result = substituteMacros(sql, { filter: '' }, defaultTimeRange)
      expect(result).toBe("SELECT * FROM logs WHERE filter = ''")
    })
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
      expect(cell.name).toBe('Table 2')
    })

    it('should create unique name with incrementing counter', () => {
      const existingNames = new Set(['Table', 'Table 2', 'Table 3'])
      const cell = createDefaultCell('table', existingNames)
      expect(cell.name).toBe('Table 4')
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

    it('should have auto height layout', () => {
      const cell = createDefaultCell('table', new Set())
      expect(cell.layout).toEqual({ height: 'auto' })
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

  describe('unknown type fallback', () => {
    it('should default to table type for unknown types', () => {
      // TypeScript would prevent this, but testing runtime behavior
      const cell = createDefaultCell('unknown' as never, new Set())
      expect(cell.type).toBe('table')
      expect((cell as { sql: string }).sql).toBe(DEFAULT_SQL.table)
    })
  })
})
