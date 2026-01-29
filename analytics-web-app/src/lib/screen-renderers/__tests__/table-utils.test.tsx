import { render, screen } from '@testing-library/react'
import { DataType, Timestamp, TimeUnit } from 'apache-arrow'
import { expandRowMacros, OverrideCell, TableColumn } from '../table-utils'

describe('expandRowMacros', () => {
  describe('dot notation', () => {
    it('should expand a single $row.column macro', () => {
      const template = '[View](/process?id=$row.process_id)'
      const row = { process_id: '123', name: 'test' }
      expect(expandRowMacros(template, row)).toBe('[View](/process?id=123)')
    })

    it('should expand multiple $row.column macros', () => {
      const template = '[View $row.name](/process?id=$row.process_id)'
      const row = { process_id: '123', name: 'MyProcess' }
      expect(expandRowMacros(template, row)).toBe('[View MyProcess](/process?id=123)')
    })

    it('should handle alphanumeric column names with underscores', () => {
      const template = '$row.process_id_123'
      const row = { process_id_123: 'value' }
      expect(expandRowMacros(template, row)).toBe('value')
    })

    it('should replace missing columns with empty string', () => {
      const template = '[View](/process?id=$row.missing)'
      const row = { process_id: '123' }
      expect(expandRowMacros(template, row)).toBe('[View](/process?id=)')
    })

    it('should handle null values as empty string', () => {
      const template = '[View](/process?id=$row.process_id)'
      const row = { process_id: null }
      expect(expandRowMacros(template, row)).toBe('[View](/process?id=)')
    })

    it('should handle undefined values as empty string', () => {
      const template = '[View](/process?id=$row.process_id)'
      const row = { process_id: undefined }
      expect(expandRowMacros(template, row)).toBe('[View](/process?id=)')
    })
  })

  describe('bracket notation', () => {
    it('should expand $row["column-name"] with double quotes', () => {
      const template = '[View](/details?id=$row["process-id"])'
      const row = { 'process-id': '456' }
      expect(expandRowMacros(template, row)).toBe('[View](/details?id=456)')
    })

    it("should expand $row['column-name'] with single quotes", () => {
      const template = "[View](/details?id=$row['process-id'])"
      const row = { 'process-id': '789' }
      expect(expandRowMacros(template, row)).toBe('[View](/details?id=789)')
    })

    it('should handle column names with spaces', () => {
      const template = '$row["Display Name"]'
      const row = { 'Display Name': 'My Process' }
      expect(expandRowMacros(template, row)).toBe('My Process')
    })

    it('should handle column names with special characters', () => {
      const template = '$row["col.with.dots"]'
      const row = { 'col.with.dots': 'value' }
      expect(expandRowMacros(template, row)).toBe('value')
    })

    it('should replace missing bracket notation columns with empty string', () => {
      const template = '$row["missing-column"]'
      const row = { other: 'value' }
      expect(expandRowMacros(template, row)).toBe('')
    })
  })

  describe('mixed notation', () => {
    it('should handle both dot and bracket notation in same template', () => {
      const template = '[View](/details?id=$row["process-id"]&name=$row.name)'
      const row = { 'process-id': '123', name: 'Test' }
      expect(expandRowMacros(template, row)).toBe('[View](/details?id=123&name=Test)')
    })

    it('should process bracket notation before dot notation', () => {
      // This ensures bracket notation is processed first so $row.x inside brackets works
      const template = '$row["key"]$row.simple'
      const row = { key: 'A', simple: 'B' }
      expect(expandRowMacros(template, row)).toBe('AB')
    })
  })

  describe('edge cases', () => {
    it('should handle template with no macros', () => {
      const template = '[Static Link](/path)'
      const row = { id: '123' }
      expect(expandRowMacros(template, row)).toBe('[Static Link](/path)')
    })

    it('should handle empty template', () => {
      expect(expandRowMacros('', { id: '123' })).toBe('')
    })

    it('should handle empty row', () => {
      const template = '$row.id'
      expect(expandRowMacros(template, {})).toBe('')
    })

    it('should convert non-string values to strings', () => {
      const template = '$row.count, $row.active'
      const row = { count: 42, active: true }
      expect(expandRowMacros(template, row)).toBe('42, true')
    })

    it('should handle numeric values', () => {
      const template = '/details?id=$row.id&count=$row.count'
      const row = { id: 123, count: 456 }
      expect(expandRowMacros(template, row)).toBe('/details?id=123&count=456')
    })
  })
})

describe('OverrideCell', () => {
  // Helper to create minimal columns array
  const stringColumns: TableColumn[] = [
    { name: 'id', type: new DataType() },
    { name: 'name', type: new DataType() },
    { name: 'other', type: new DataType() },
    { name: 'process-id', type: new DataType() },
  ]

  it('should render a simple link with expanded macros', () => {
    render(<OverrideCell format="[View](/process?id=$row.id)" row={{ id: '123' }} columns={stringColumns} />)
    const link = screen.getByRole('link', { name: 'View' })
    expect(link).toHaveAttribute('href', '/process?id=123')
  })

  it('should render expanded macro in link text', () => {
    render(
      <OverrideCell
        format="[$row.name](/process?id=$row.id)"
        row={{ id: '123', name: 'MyApp' }}
        columns={stringColumns}
      />
    )
    const link = screen.getByRole('link', { name: 'MyApp' })
    expect(link).toHaveAttribute('href', '/process?id=123')
  })

  it('should render multiple links', () => {
    render(
      <OverrideCell
        format="[View](/view?id=$row.id) | [Edit](/edit?id=$row.id)"
        row={{ id: '456' }}
        columns={stringColumns}
      />
    )
    const links = screen.getAllByRole('link')
    expect(links).toHaveLength(2)
    expect(links[0]).toHaveAttribute('href', '/view?id=456')
    expect(links[1]).toHaveAttribute('href', '/edit?id=456')
  })

  it('should handle missing columns gracefully', () => {
    render(
      <OverrideCell format="[View](/process?id=$row.missing)" row={{ other: 'value' }} columns={stringColumns} />
    )
    const link = screen.getByRole('link', { name: 'View' })
    expect(link).toHaveAttribute('href', '/process?id=')
  })

  it('should render plain text when no markdown link', () => {
    render(<OverrideCell format="ID: $row.id" row={{ id: '789' }} columns={stringColumns} />)
    expect(screen.getByText('ID: 789')).toBeInTheDocument()
  })

  it('should handle bracket notation in links', () => {
    render(
      <OverrideCell
        format='[View](/details?id=$row["process-id"])'
        row={{ 'process-id': 'abc' }}
        columns={stringColumns}
      />
    )
    const link = screen.getByRole('link', { name: 'View' })
    expect(link).toHaveAttribute('href', '/details?id=abc')
  })
})

describe('expandRowMacros with timestamps', () => {
  it('should format timestamp columns as RFC3339 (ISO 8601)', () => {
    // Create a timestamp type (microseconds)
    const timestampType = new Timestamp(TimeUnit.MICROSECOND, null)
    const columnTypes = new Map<string, DataType>([['start_time', timestampType]])

    // Value in microseconds since epoch (2024-01-15T10:30:00.000Z)
    const microsSinceEpoch = BigInt(1705314600000000)

    const template = '/process?from=$row.start_time'
    const row = { start_time: microsSinceEpoch }

    const result = expandRowMacros(template, row, columnTypes)
    expect(result).toBe('/process?from=2024-01-15T10:30:00.000Z')
  })

  it('should format timestamp with bracket notation as RFC3339', () => {
    const timestampType = new Timestamp(TimeUnit.MICROSECOND, null)
    const columnTypes = new Map<string, DataType>([['start-time', timestampType]])

    const microsSinceEpoch = BigInt(1705314600000000)

    const template = '/process?from=$row["start-time"]'
    const row = { 'start-time': microsSinceEpoch }

    const result = expandRowMacros(template, row, columnTypes)
    expect(result).toBe('/process?from=2024-01-15T10:30:00.000Z')
  })

  it('should handle non-timestamp columns as plain strings', () => {
    const timestampType = new Timestamp(TimeUnit.MICROSECOND, null)
    const columnTypes = new Map<string, DataType>([['start_time', timestampType]])

    const template = '/process?id=$row.process_id&from=$row.start_time'
    const row = { process_id: 'abc123', start_time: BigInt(1705314600000000) }

    const result = expandRowMacros(template, row, columnTypes)
    expect(result).toBe('/process?id=abc123&from=2024-01-15T10:30:00.000Z')
  })

  it('should work without column types (backwards compatible)', () => {
    const template = '/process?id=$row.id'
    const row = { id: '123' }

    // No column types provided - should still work
    const result = expandRowMacros(template, row)
    expect(result).toBe('/process?id=123')
  })
})
