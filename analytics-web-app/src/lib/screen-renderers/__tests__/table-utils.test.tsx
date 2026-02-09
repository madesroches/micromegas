import { render, screen, fireEvent } from '@testing-library/react'
import { DataType, Timestamp, TimeUnit } from 'apache-arrow'
import {
  expandRowMacros,
  expandVariableMacros,
  extractMacroColumns,
  findUnknownMacros,
  validateFormatMacros,
  OverrideCell,
  TableColumn,
  SortHeader,
  HiddenColumnsBar,
  getNextSortState,
} from '../table-utils'

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

describe('expandVariableMacros', () => {
  it('should expand a single variable', () => {
    const template = '[Search: $search](/results?q=$search)'
    const variables = { search: 'test query' }
    expect(expandVariableMacros(template, variables)).toBe('[Search: test query](/results?q=test query)')
  })

  it('should expand multiple variables', () => {
    const template = '/metrics?name=$metric&host=$host'
    const variables = { metric: 'cpu_usage', host: 'server1' }
    expect(expandVariableMacros(template, variables)).toBe('/metrics?name=cpu_usage&host=server1')
  })

  it('should not expand unknown variables', () => {
    const template = '/path?id=$unknown'
    const variables = { known: 'value' }
    expect(expandVariableMacros(template, variables)).toBe('/path?id=$unknown')
  })

  it('should handle longer variable names first to avoid partial matches', () => {
    const template = '$metric and $metric_name'
    const variables = { metric: 'cpu', metric_name: 'CPU Usage' }
    expect(expandVariableMacros(template, variables)).toBe('cpu and CPU Usage')
  })

  it('should return unchanged template when no variables provided', () => {
    const template = '/path?id=$var'
    expect(expandVariableMacros(template, {})).toBe('/path?id=$var')
  })

  it('should respect word boundaries', () => {
    const template = '$metrics vs $metric'
    const variables = { metric: 'cpu' }
    expect(expandVariableMacros(template, variables)).toBe('$metrics vs cpu')
  })
})

describe('extractMacroColumns', () => {
  it('should extract dot notation columns', () => {
    const template = '[View](/process?id=$row.process_id&name=$row.name)'
    expect(extractMacroColumns(template)).toEqual(['process_id', 'name'])
  })

  it('should extract bracket notation columns', () => {
    const template = '[View](/details?id=$row["process-id"]&name=$row["Display Name"])'
    expect(extractMacroColumns(template)).toEqual(['process-id', 'Display Name'])
  })

  it('should extract mixed notation columns', () => {
    const template = '[View](/details?id=$row["process-id"]&name=$row.name)'
    const result = extractMacroColumns(template)
    expect(result).toHaveLength(2)
    expect(result).toContain('process-id')
    expect(result).toContain('name')
  })

  it('should return unique columns only', () => {
    const template = '[View](/process?id=$row.id&other=$row.id)'
    expect(extractMacroColumns(template)).toEqual(['id'])
  })

  it('should return empty array for template with no macros', () => {
    const template = '[Static Link](/path)'
    expect(extractMacroColumns(template)).toEqual([])
  })

  it('should return empty array for empty template', () => {
    expect(extractMacroColumns('')).toEqual([])
  })
})

describe('findUnknownMacros', () => {
  it('should find unknown $name style macros', () => {
    const template = '[View](/process?id=$missing)'
    expect(findUnknownMacros(template, [])).toEqual(['$missing'])
  })

  it('should find multiple unknown macros', () => {
    const template = '[View](/process?id=$id&name=$name)'
    expect(findUnknownMacros(template, [])).toEqual(['$id', '$name'])
  })

  it('should not flag $row as unknown', () => {
    const template = '[View](/process?id=$row.id)'
    expect(findUnknownMacros(template, [])).toEqual([])
  })

  it('should not flag $begin and $end as unknown', () => {
    const template = '[View](/process?from=$begin&to=$end)'
    expect(findUnknownMacros(template, [])).toEqual([])
  })

  it('should not flag known variables as unknown', () => {
    const template = '[View](/process?id=$process_id&name=$metric)'
    expect(findUnknownMacros(template, ['process_id', 'metric'])).toEqual([])
  })

  it('should return empty array for template with no macros', () => {
    const template = '[Static Link](/path)'
    expect(findUnknownMacros(template, [])).toEqual([])
  })

  it('should handle mixed known and unknown macros', () => {
    const template = '[View](/process?id=$known&other=$missing)'
    expect(findUnknownMacros(template, ['known'])).toEqual(['$missing'])
  })
})

describe('validateFormatMacros', () => {
  it('should return empty result when all columns exist and syntax is valid', () => {
    const template = '[View](/process?id=$row.id&name=$row.name)'
    const availableColumns = ['id', 'name', 'other']
    const result = validateFormatMacros(template, availableColumns)
    expect(result.missingColumns).toEqual([])
    expect(result.unknownMacros).toEqual([])
  })

  it('should return missing dot notation columns', () => {
    const template = '[View](/process?id=$row.missing)'
    const availableColumns = ['id', 'name']
    const result = validateFormatMacros(template, availableColumns)
    expect(result.missingColumns).toEqual(['missing'])
    expect(result.unknownMacros).toEqual([])
  })

  it('should return missing bracket notation columns', () => {
    const template = '[View](/details?id=$row["missing-column"])'
    const availableColumns = ['id', 'name']
    const result = validateFormatMacros(template, availableColumns)
    expect(result.missingColumns).toEqual(['missing-column'])
    expect(result.unknownMacros).toEqual([])
  })

  it('should return multiple missing columns', () => {
    const template = '[View](/process?a=$row.missing1&b=$row["missing-2"])'
    const availableColumns = ['id', 'name']
    const result = validateFormatMacros(template, availableColumns)
    expect(result.missingColumns).toHaveLength(2)
    expect(result.missingColumns).toContain('missing1')
    expect(result.missingColumns).toContain('missing-2')
    expect(result.unknownMacros).toEqual([])
  })

  it('should return empty result for template with no macros', () => {
    const template = '[Static Link](/path)'
    const availableColumns = ['id', 'name']
    const result = validateFormatMacros(template, availableColumns)
    expect(result.missingColumns).toEqual([])
    expect(result.unknownMacros).toEqual([])
  })

  it('should return unknown macros when not a known variable', () => {
    const template = '[View](/process?id=$missing)'
    const availableColumns = ['id', 'name']
    const result = validateFormatMacros(template, availableColumns)
    expect(result.missingColumns).toEqual([])
    expect(result.unknownMacros).toEqual(['$missing'])
  })

  it('should not flag known variables as unknown', () => {
    const template = '[View](/process?id=$process_id)'
    const availableColumns = ['id', 'name']
    const availableVariables = ['process_id']
    const result = validateFormatMacros(template, availableColumns, availableVariables)
    expect(result.missingColumns).toEqual([])
    expect(result.unknownMacros).toEqual([])
  })

  it('should return both missing columns and unknown macros', () => {
    const template = '[View](/process?id=$row.missing&other=$invalid)'
    const availableColumns = ['id', 'name']
    const result = validateFormatMacros(template, availableColumns)
    expect(result.missingColumns).toEqual(['missing'])
    expect(result.unknownMacros).toEqual(['$invalid'])
  })

  it('should not flag $begin and $end as unknown', () => {
    const template = '[View](/process?from=$begin&to=$end)'
    const availableColumns = ['id', 'name']
    const result = validateFormatMacros(template, availableColumns)
    expect(result.missingColumns).toEqual([])
    expect(result.unknownMacros).toEqual([])
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

  it('should expand notebook variables', () => {
    render(
      <OverrideCell
        format="[Search: $search](/results?q=$search&id=$row.id)"
        row={{ id: '123' }}
        columns={stringColumns}
        variables={{ search: 'test query' }}
      />
    )
    const link = screen.getByRole('link', { name: 'Search: test query' })
    expect(link).toHaveAttribute('href', '/results?q=test query&id=123')
  })

  it('should expand variables before row macros', () => {
    render(
      <OverrideCell
        format="[$metric on $row.name](/metrics?name=$metric)"
        row={{ name: 'server1' }}
        columns={stringColumns}
        variables={{ metric: 'cpu_usage' }}
      />
    )
    const link = screen.getByRole('link', { name: 'cpu_usage on server1' })
    expect(link).toHaveAttribute('href', '/metrics?name=cpu_usage')
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

// =============================================================================
// getNextSortState
// =============================================================================

describe('getNextSortState', () => {
  it('should return ASC when clicking a new column', () => {
    const result = getNextSortState('name', undefined, undefined)
    expect(result).toEqual({ sortColumn: 'name', sortDirection: 'asc' })
  })

  it('should return ASC when clicking a different column', () => {
    const result = getNextSortState('name', 'id', 'desc')
    expect(result).toEqual({ sortColumn: 'name', sortDirection: 'asc' })
  })

  it('should cycle ASC to DESC on same column', () => {
    const result = getNextSortState('name', 'name', 'asc')
    expect(result).toEqual({ sortColumn: 'name', sortDirection: 'desc' })
  })

  it('should cycle DESC to none on same column', () => {
    const result = getNextSortState('name', 'name', 'desc')
    expect(result).toEqual({ sortColumn: undefined, sortDirection: undefined })
  })
})

// =============================================================================
// SortHeader
// =============================================================================

describe('SortHeader', () => {
  const renderInTable = (ui: React.ReactElement) =>
    render(
      <table>
        <thead>
          <tr>{ui}</tr>
        </thead>
      </table>
    )

  it('should render column name', () => {
    renderInTable(
      <SortHeader columnName="id" onSort={jest.fn()}>
        id
      </SortHeader>
    )
    expect(screen.getByText('id')).toBeInTheDocument()
  })

  it('should call onSort on left-click', () => {
    const onSort = jest.fn()
    renderInTable(
      <SortHeader columnName="name" onSort={onSort}>
        name
      </SortHeader>
    )
    fireEvent.click(screen.getByRole('columnheader'))
    expect(onSort).toHaveBeenCalledWith('name')
  })

  it('should call onSort on left-click even when onHide is provided', () => {
    const onSort = jest.fn()
    const onHide = jest.fn()
    renderInTable(
      <SortHeader columnName="name" onSort={onSort} onHide={onHide}>
        name
      </SortHeader>
    )
    fireEvent.click(screen.getByRole('columnheader'))
    expect(onSort).toHaveBeenCalledWith('name')
    expect(onHide).not.toHaveBeenCalled()
  })

  it('should not open context menu on left-click when onHide is provided', () => {
    const onSort = jest.fn()
    const onHide = jest.fn()
    renderInTable(
      <SortHeader columnName="name" onSort={onSort} onHide={onHide}>
        name
      </SortHeader>
    )
    fireEvent.click(screen.getByRole('columnheader'))
    // Context menu items should not appear on left-click
    expect(screen.queryByText('Hide Column')).not.toBeInTheDocument()
    expect(screen.queryByText('Sort Ascending')).not.toBeInTheDocument()
  })

  it('should render sort indicator when active ascending', () => {
    renderInTable(
      <SortHeader columnName="name" sortColumn="name" sortDirection="asc" onSort={jest.fn()}>
        name
      </SortHeader>
    )
    const th = screen.getByRole('columnheader')
    expect(th.className).toContain('text-theme-text-primary')
  })

  it('should render sort indicator when active descending', () => {
    renderInTable(
      <SortHeader columnName="name" sortColumn="name" sortDirection="desc" onSort={jest.fn()}>
        name
      </SortHeader>
    )
    const th = screen.getByRole('columnheader')
    expect(th.className).toContain('text-theme-text-primary')
  })

  it('should render muted style when not active', () => {
    renderInTable(
      <SortHeader columnName="name" sortColumn="other" sortDirection="asc" onSort={jest.fn()}>
        name
      </SortHeader>
    )
    const th = screen.getByRole('columnheader')
    expect(th.className).toContain('text-theme-text-muted')
  })
})

// =============================================================================
// HiddenColumnsBar
// =============================================================================

describe('HiddenColumnsBar', () => {
  it('should render nothing when no columns are hidden', () => {
    const { container } = render(<HiddenColumnsBar hiddenColumns={[]} onRestore={jest.fn()} />)
    expect(container.firstChild).toBeNull()
  })

  it('should render a pill for each hidden column', () => {
    render(<HiddenColumnsBar hiddenColumns={['col_a', 'col_b']} onRestore={jest.fn()} />)
    expect(screen.getByText('col_a')).toBeInTheDocument()
    expect(screen.getByText('col_b')).toBeInTheDocument()
  })

  it('should call onRestore when clicking a pill', () => {
    const onRestore = jest.fn()
    render(<HiddenColumnsBar hiddenColumns={['col_a', 'col_b']} onRestore={onRestore} />)
    fireEvent.click(screen.getByText('col_a'))
    expect(onRestore).toHaveBeenCalledWith('col_a')
  })

  it('should show "Show all" button when more than one column is hidden and onRestoreAll is provided', () => {
    const onRestoreAll = jest.fn()
    render(
      <HiddenColumnsBar hiddenColumns={['col_a', 'col_b']} onRestore={jest.fn()} onRestoreAll={onRestoreAll} />
    )
    const showAll = screen.getByText('Show all')
    expect(showAll).toBeInTheDocument()
    fireEvent.click(showAll)
    expect(onRestoreAll).toHaveBeenCalled()
  })

  it('should not show "Show all" when only one column is hidden', () => {
    render(
      <HiddenColumnsBar hiddenColumns={['col_a']} onRestore={jest.fn()} onRestoreAll={jest.fn()} />
    )
    expect(screen.queryByText('Show all')).not.toBeInTheDocument()
  })

  it('should not show "Show all" when onRestoreAll is not provided', () => {
    render(<HiddenColumnsBar hiddenColumns={['col_a', 'col_b']} onRestore={jest.fn()} />)
    expect(screen.queryByText('Show all')).not.toBeInTheDocument()
  })

  it('should render "Hidden:" label', () => {
    render(<HiddenColumnsBar hiddenColumns={['col_a']} onRestore={jest.fn()} />)
    expect(screen.getByText('Hidden:')).toBeInTheDocument()
  })
})
