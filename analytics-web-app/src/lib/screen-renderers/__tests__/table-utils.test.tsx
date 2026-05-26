// Mock matchMedia and cell-registry to avoid heavyweight imports in this file.
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

import { useMemo, useState } from 'react'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { DataType, Timestamp, TimeUnit } from 'apache-arrow'
import { renderHook, act } from '@testing-library/react'
import {
  extractMacroColumns,
  findUnknownMacros,
  validateFormatMacros,
  OverrideCell,
  TableColumn,
  SortHeader,
  HiddenColumnsBar,
  RowContextMenu,
  getNextSortState,
  useRowManagement,
  ColumnOverride,
  TableBody,
} from '../table-utils'
import { ColumnHeaderWarningIcon, useColumnWarnings, WarningReporterContext } from '../warning-reporter'

// =============================================================================
// Validation helpers (still public)
// =============================================================================

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

  it('should not flag $from and $to as unknown', () => {
    const template = '[View](/process?from=$from&to=$to)'
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

  it('should not flag cell selection names as unknown', () => {
    const template = '[View](/process?from=$upstream.selected.start_time)'
    expect(findUnknownMacros(template, [], ['upstream'])).toEqual([])
  })

  it('should flag unknown names even with cell selections provided', () => {
    const template = '[View](/process?from=$unknown.selected.start_time)'
    expect(findUnknownMacros(template, [], ['upstream'])).toEqual(['$unknown'])
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

  it('should not flag $from and $to as unknown', () => {
    const template = '[View](/process?from=$from&to=$to)'
    const availableColumns = ['id', 'name']
    const result = validateFormatMacros(template, availableColumns)
    expect(result.missingColumns).toEqual([])
    expect(result.unknownMacros).toEqual([])
  })
})

// =============================================================================
// OverrideCell — exercises the new evaluateTemplate path
// =============================================================================

const stringColumns: TableColumn[] = [
  { name: 'id', type: new DataType() },
  { name: 'name', type: new DataType() },
  { name: 'other', type: new DataType() },
  { name: 'process-id', type: new DataType() },
]

const noCells = { cellSelections: {}, cellResults: {} }

describe('OverrideCell — basic rendering', () => {
  it('renders a link with $row.col expanded', () => {
    render(
      <OverrideCell
        format="[View](/process?id=$row.id)"
        columnName="link"
        row={{ id: '123' }}
        columns={stringColumns}
        {...noCells}
      />,
    )
    const link = screen.getByRole('link', { name: 'View' })
    expect(link).toHaveAttribute('href', '/process?id=123')
  })

  it('expands $row.col in both link text and href', () => {
    render(
      <OverrideCell
        format="[$row.name](/process?id=$row.id)"
        columnName="link"
        row={{ id: '123', name: 'MyApp' }}
        columns={stringColumns}
        {...noCells}
      />,
    )
    const link = screen.getByRole('link', { name: 'MyApp' })
    expect(link).toHaveAttribute('href', '/process?id=123')
  })

  it('expands bracket-notation row macros', () => {
    render(
      <OverrideCell
        format='[View](/details?id=$row["process-id"])'
        columnName="link"
        row={{ 'process-id': 'abc' }}
        columns={stringColumns}
        {...noCells}
      />,
    )
    const link = screen.getByRole('link', { name: 'View' })
    expect(link).toHaveAttribute('href', '/details?id=abc')
  })

  it('expands notebook variables', () => {
    render(
      <OverrideCell
        format="[Search: $search](/results?q=$search&id=$row.id)"
        columnName="link"
        row={{ id: '123' }}
        columns={stringColumns}
        variables={{ search: 'test query' }}
        {...noCells}
      />,
    )
    const link = screen.getByRole('link', { name: 'Search: test query' })
    expect(link).toHaveAttribute('href', '/results?q=test query&id=123')
  })

  it('expands $from and $to time range macros', () => {
    render(
      <OverrideCell
        format="[View](/details?from=$from&to=$to&id=$row.id)"
        columnName="link"
        row={{ id: '123' }}
        columns={stringColumns}
        timeRange={{ begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' }}
        {...noCells}
      />,
    )
    const link = screen.getByRole('link', { name: 'View' })
    expect(link).toHaveAttribute(
      'href',
      '/details?from=2024-01-01T00:00:00Z&to=2024-01-02T00:00:00Z&id=123',
    )
  })

  it('expands $cell.selected.col macros', () => {
    render(
      <OverrideCell
        format="[View](/details?from=$upstream.selected.start_time&to=$upstream.selected.end_time)"
        columnName="link"
        row={{ id: '123' }}
        columns={stringColumns}
        cellSelections={{
          upstream: {
            start_time: '2024-01-01T00:00:00Z',
            end_time: '2024-01-02T00:00:00Z',
          },
        }}
        cellResults={{}}
      />,
    )
    const link = screen.getByRole('link', { name: 'View' })
    expect(link).toHaveAttribute('href', '/details?from=2024-01-01T00:00:00Z&to=2024-01-02T00:00:00Z')
  })
})

// =============================================================================
// Phase 4 step 15 — column-header warning surface
// =============================================================================

// The harness component MUST receive a stable `overrides` reference — otherwise
// `useColumnWarnings`'s reset effect re-fires every render and trips React's
// "Too many re-renders" guard (see warning-reporter.tsx). Tests therefore
// memoize their `overrides` array via `useMemo`.
function TableHarness({
  overrides,
  row,
  variables = {},
}: {
  overrides: ColumnOverride[]
  row: Record<string, unknown>
  variables?: Record<string, string>
}) {
  const { columnWarnings, reportWarning } = useColumnWarnings(overrides)
  const data = useMemo(() => ({ numRows: 1, get: () => row }), [row])
  const columns: TableColumn[] = useMemo(
    () => [{ name: 'value', type: new DataType() }],
    [],
  )
  return (
    <WarningReporterContext.Provider value={reportWarning}>
      <table>
        <thead>
          <tr>
            {columns.map((col) => {
              const w = columnWarnings.get(col.name)
              return (
                <th key={col.name} data-testid={`th-${col.name}`}>
                  {col.name}
                  {w?.size ? <ColumnHeaderWarningIcon warnings={[...w]} /> : null}
                </th>
              )
            })}
          </tr>
        </thead>
        <TableBody
          data={data}
          columns={columns}
          allColumns={columns}
          overrides={overrides}
          variables={variables}
          cellSelections={{}}
          cellResults={{}}
        />
      </table>
    </WarningReporterContext.Provider>
  )
}

describe('OverrideCell + column warning surface', () => {
  it('(a) renders format_value($row.bytes, "bytes") with adaptive output', () => {
    function Harness() {
      const overrides = useMemo(
        () => [{ column: 'value', format: "format_value($row.bytes, 'bytes')" }],
        [],
      )
      return <TableHarness overrides={overrides} row={{ bytes: 3678630912 }} />
    }
    render(<Harness />)
    expect(screen.getByText('3.4 GB')).toBeInTheDocument()
    const th = screen.getByTestId('th-value')
    expect(th.querySelector('[title]')).toBeNull()
  })

  it('(b) renders literal source AND surfaces a warning icon when arg is unresolved', async () => {
    function Harness() {
      const overrides = useMemo(
        () => [{ column: 'value', format: "format_value($missing, 'bytes')" }],
        [],
      )
      return <TableHarness overrides={overrides} row={{ bytes: 100 }} />
    }
    render(<Harness />)
    expect(screen.getByText("format_value($missing, 'bytes')")).toBeInTheDocument()
    const th = screen.getByTestId('th-value')
    await waitFor(() => {
      expect(th.querySelector('[title]')).not.toBeNull()
    })
    expect(th.querySelector('[title]')!.getAttribute('title')).toContain(
      'format_value: $missing is unresolved',
    )
  })

  it('(d) clears the warning icon when the override changes to a valid format', async () => {
    function Wrapper() {
      const [valid, setValid] = useState(false)
      const overrides = useMemo<ColumnOverride[]>(
        () =>
          valid
            ? [{ column: 'value', format: 'plain' }]
            : [{ column: 'value', format: "format_value($missing, 'bytes')" }],
        [valid],
      )
      return (
        <>
          <button onClick={() => setValid(true)}>swap</button>
          <TableHarness overrides={overrides} row={{ bytes: 100 }} />
        </>
      )
    }

    render(<Wrapper />)
    const th = screen.getByTestId('th-value')
    await waitFor(() => {
      expect(th.querySelector('[title]')).not.toBeNull()
    })
    fireEvent.click(screen.getByText('swap'))
    await waitFor(() => {
      expect(th.querySelector('[title]')).toBeNull()
    })
  })

  it('(d2) replaces the warning when the override changes from one bad format to a different bad format', async () => {
    // Regression: a previous useEffect-based reset in `useColumnWarnings` ran
    // *after* child OverrideCell effects in the same commit and clobbered any
    // new warnings the children had just posted. Because the children's
    // useEffect deps were stable on the next render, they wouldn't re-post —
    // so editing from `format_value($missing, …)` to `format_value($other, …)`
    // silently dropped the icon.
    function Wrapper() {
      const [swapped, setSwapped] = useState(false)
      const overrides = useMemo<ColumnOverride[]>(
        () =>
          swapped
            ? [{ column: 'value', format: "format_value($other, 'bytes')" }]
            : [{ column: 'value', format: "format_value($missing, 'bytes')" }],
        [swapped],
      )
      return (
        <>
          <button onClick={() => setSwapped(true)}>swap</button>
          <TableHarness overrides={overrides} row={{ bytes: 100 }} />
        </>
      )
    }

    render(<Wrapper />)
    const th = screen.getByTestId('th-value')
    await waitFor(() => {
      expect(th.querySelector('[title]')!.getAttribute('title')).toContain(
        'format_value: $missing is unresolved',
      )
    })
    fireEvent.click(screen.getByText('swap'))
    await waitFor(() => {
      expect(th.querySelector('[title]')!.getAttribute('title')).toContain(
        'format_value: $other is unresolved',
      )
    })
    // And the stale warning is gone (the reset still works).
    expect(th.querySelector('[title]')!.getAttribute('title')).not.toContain(
      '$missing',
    )
  })

  it('(e) preserves naked $cell.selected.col as source and surfaces the icon (pins §6 #2)', async () => {
    function Harness() {
      const overrides = useMemo(
        () => [{ column: 'value', format: 'value: $upstream.selected.col' }],
        [],
      )
      return <TableHarness overrides={overrides} row={{ bytes: 100 }} />
    }
    render(<Harness />)
    expect(screen.getByText('value: $upstream.selected.col')).toBeInTheDocument()
    const th = screen.getByTestId('th-value')
    await waitFor(() => {
      expect(th.querySelector('[title]')).not.toBeNull()
    })
    expect(th.querySelector('[title]')!.getAttribute('title')).toContain(
      '$upstream.selected.col is unresolved',
    )
  })

  it('(f) preserves $row.col source when the value is null and surfaces the icon (pins §6 #4)', async () => {
    function Harness() {
      const overrides = useMemo(
        () => [{ column: 'value', format: 'val=$row.maybe' }],
        [],
      )
      return <TableHarness overrides={overrides} row={{ maybe: null }} />
    }
    render(<Harness />)
    expect(screen.getByText('val=$row.maybe')).toBeInTheDocument()
    const th = screen.getByTestId('th-value')
    await waitFor(() => {
      expect(th.querySelector('[title]')).not.toBeNull()
    })
  })

  it('(g) preserves $metric.unit source when metric is a simple-string variable (pins §6 #3)', async () => {
    function Harness() {
      const overrides = useMemo(() => [{ column: 'value', format: '$metric.unit' }], [])
      return <TableHarness overrides={overrides} row={{}} variables={{ metric: 'cpu' }} />
    }
    render(<Harness />)
    expect(screen.getByText('$metric.unit')).toBeInTheDocument()
    const th = screen.getByTestId('th-value')
    await waitFor(() => {
      expect(th.querySelector('[title]')).not.toBeNull()
    })
  })

  it('(h) renders a hidden timestamp column as RFC3339 via allColumns', () => {
    const timestampType = new Timestamp(TimeUnit.MICROSECOND, null)
    const visibleColumns: TableColumn[] = [{ name: 'name', type: new DataType() }]
    const allColumns: TableColumn[] = [
      { name: 'name', type: new DataType() },
      { name: 'start_time', type: timestampType },
    ]
    const microsSinceEpoch = BigInt(1705314600000000)

    render(
      <OverrideCell
        format="Started: $row.start_time"
        columnName="name"
        row={{ name: 'server1', start_time: microsSinceEpoch }}
        columns={visibleColumns}
        allColumns={allColumns}
        cellSelections={{}}
        cellResults={{}}
      />,
    )
    expect(screen.getByText('Started: 2024-01-15T10:30:00.000Z')).toBeInTheDocument()
  })

  it('survives an unmemoized overrides array (content-hash robustness)', async () => {
    // Without content-hashing the hook would treat each fresh-ref `overrides`
    // array as a change, re-fire the reset effect every render, and trip
    // React's "Too many re-renders" guard. With hashing this passes.
    function Harness() {
      // Deliberately UNmemoized: fresh array reference on every render.
      const overrides = [{ column: 'value', format: "format_value($missing, 'bytes')" }]
      return <TableHarness overrides={overrides} row={{ bytes: 100 }} />
    }
    render(<Harness />)
    const th = screen.getByTestId('th-value')
    await waitFor(() => {
      expect(th.querySelector('[title]')).not.toBeNull()
    })
  })

  it('(c) dedups the same warning produced by many rows into a single tooltip entry', async () => {
    function MultiRowHarness() {
      const overrides = useMemo(
        () => [{ column: 'value', format: "format_value($missing, 'bytes')" }],
        [],
      )
      const { columnWarnings, reportWarning } = useColumnWarnings(overrides)
      const columns: TableColumn[] = useMemo(
        () => [{ name: 'value', type: new DataType() }],
        [],
      )
      const data = useMemo(
        () => ({ numRows: 5, get: (i: number) => ({ value: i }) }),
        [],
      )
      return (
        <WarningReporterContext.Provider value={reportWarning}>
          <table>
            <thead>
              <tr>
                <th data-testid="th-value">
                  value
                  {columnWarnings.get('value')?.size ? (
                    <ColumnHeaderWarningIcon warnings={[...columnWarnings.get('value')!]} />
                  ) : null}
                </th>
              </tr>
            </thead>
            <TableBody
              data={data}
              columns={columns}
              allColumns={columns}
              overrides={overrides}
              cellSelections={{}}
              cellResults={{}}
            />
          </table>
        </WarningReporterContext.Provider>
      )
    }
    render(<MultiRowHarness />)
    const th = screen.getByTestId('th-value')
    await waitFor(() => {
      expect(th.querySelector('[title]')).not.toBeNull()
    })
    const titleAttr = th.querySelector('[title]')!.getAttribute('title')!
    expect(titleAttr.split('\n').length).toBe(1)
    expect(titleAttr).toBe('format_value: $missing is unresolved')
  })
})

// =============================================================================
// Existing utilities (unchanged) — getNextSortState, SortHeader, etc.
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

describe('SortHeader', () => {
  const renderInTable = (ui: React.ReactElement) =>
    render(
      <table>
        <thead>
          <tr>{ui}</tr>
        </thead>
      </table>,
    )

  it('should render column name', () => {
    renderInTable(
      <SortHeader columnName="id" onSort={jest.fn()}>
        id
      </SortHeader>,
    )
    expect(screen.getByText('id')).toBeInTheDocument()
  })

  it('should call onSort on left-click', () => {
    const onSort = jest.fn()
    renderInTable(
      <SortHeader columnName="name" onSort={onSort}>
        name
      </SortHeader>,
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
      </SortHeader>,
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
      </SortHeader>,
    )
    fireEvent.click(screen.getByRole('columnheader'))
    expect(screen.queryByText('Hide Column')).not.toBeInTheDocument()
    expect(screen.queryByText('Sort Ascending')).not.toBeInTheDocument()
  })

  it('should render sort indicator when active ascending', () => {
    renderInTable(
      <SortHeader columnName="name" sortColumn="name" sortDirection="asc" onSort={jest.fn()}>
        name
      </SortHeader>,
    )
    const th = screen.getByRole('columnheader')
    expect(th.className).toContain('text-theme-text-primary')
  })

  it('should render sort indicator when active descending', () => {
    renderInTable(
      <SortHeader columnName="name" sortColumn="name" sortDirection="desc" onSort={jest.fn()}>
        name
      </SortHeader>,
    )
    const th = screen.getByRole('columnheader')
    expect(th.className).toContain('text-theme-text-primary')
  })

  it('should render muted style when not active', () => {
    renderInTable(
      <SortHeader columnName="name" sortColumn="other" sortDirection="asc" onSort={jest.fn()}>
        name
      </SortHeader>,
    )
    const th = screen.getByRole('columnheader')
    expect(th.className).toContain('text-theme-text-muted')
  })

  it('renders a trailingIcon when provided', () => {
    renderInTable(
      <SortHeader columnName="name" onSort={jest.fn()} trailingIcon={<span data-testid="trail">!</span>}>
        name
      </SortHeader>,
    )
    expect(screen.getByTestId('trail')).toBeInTheDocument()
  })
})

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
      <HiddenColumnsBar hiddenColumns={['col_a', 'col_b']} onRestore={jest.fn()} onRestoreAll={onRestoreAll} />,
    )
    const showAll = screen.getByText('Show all')
    expect(showAll).toBeInTheDocument()
    fireEvent.click(showAll)
    expect(onRestoreAll).toHaveBeenCalled()
  })

  it('should not show "Show all" when only one column is hidden', () => {
    render(
      <HiddenColumnsBar hiddenColumns={['col_a']} onRestore={jest.fn()} onRestoreAll={jest.fn()} />,
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

describe('useRowManagement', () => {
  it('should return empty hiddenRows by default', () => {
    const onChange = jest.fn()
    const { result } = renderHook(() => useRowManagement({}, onChange))
    expect(result.current.hiddenRows).toEqual([])
  })

  it('should return hiddenRows from config', () => {
    const onChange = jest.fn()
    const { result } = renderHook(() => useRowManagement({ hiddenRows: ['a', 'b'] }, onChange))
    expect(result.current.hiddenRows).toEqual(['a', 'b'])
  })

  it('should hide a row', () => {
    const onChange = jest.fn()
    const { result } = renderHook(() => useRowManagement({}, onChange))
    act(() => result.current.handleHideRow('field_a'))
    expect(onChange).toHaveBeenCalledWith({ hiddenRows: ['field_a'] })
  })

  it('should not duplicate when hiding an already hidden row', () => {
    const onChange = jest.fn()
    const { result } = renderHook(() =>
      useRowManagement({ hiddenRows: ['field_a'] }, onChange),
    )
    act(() => result.current.handleHideRow('field_a'))
    expect(onChange).not.toHaveBeenCalled()
  })

  it('should restore a row', () => {
    const onChange = jest.fn()
    const { result } = renderHook(() =>
      useRowManagement({ hiddenRows: ['field_a', 'field_b'] }, onChange),
    )
    act(() => result.current.handleRestoreRow('field_a'))
    expect(onChange).toHaveBeenCalledWith({ hiddenRows: ['field_b'] })
  })

  it('should restore all rows', () => {
    const onChange = jest.fn()
    const { result } = renderHook(() =>
      useRowManagement({ hiddenRows: ['field_a', 'field_b'] }, onChange),
    )
    act(() => result.current.handleRestoreAll())
    expect(onChange).toHaveBeenCalledWith({ hiddenRows: [] })
  })

  it('should preserve other config keys when hiding', () => {
    const onChange = jest.fn()
    const { result } = renderHook(() =>
      useRowManagement({ hiddenRows: [], otherKey: 'value' }, onChange),
    )
    act(() => result.current.handleHideRow('field_a'))
    expect(onChange).toHaveBeenCalledWith({ hiddenRows: ['field_a'], otherKey: 'value' })
  })
})

describe('RowContextMenu', () => {
  const renderInTable = (ui: React.ReactElement) =>
    render(
      <table>
        <tbody>
          <tr>{ui}</tr>
        </tbody>
      </table>,
    )

  it('should render children', () => {
    renderInTable(
      <RowContextMenu rowName="field_a" onHide={jest.fn()}>
        <td>field_a</td>
      </RowContextMenu>,
    )
    expect(screen.getByText('field_a')).toBeInTheDocument()
  })

  it('should show context menu on right-click with Hide Row option', () => {
    renderInTable(
      <RowContextMenu rowName="field_a" onHide={jest.fn()}>
        <td>field_a</td>
      </RowContextMenu>,
    )
    fireEvent.contextMenu(screen.getByText('field_a'))
    expect(screen.getByText('Hide Row')).toBeInTheDocument()
  })

  it('should call onHide when Hide Row is clicked', () => {
    const onHide = jest.fn()
    renderInTable(
      <RowContextMenu rowName="field_a" onHide={onHide}>
        <td>field_a</td>
      </RowContextMenu>,
    )
    fireEvent.contextMenu(screen.getByText('field_a'))
    fireEvent.click(screen.getByText('Hide Row'))
    expect(onHide).toHaveBeenCalledWith('field_a')
  })
})
