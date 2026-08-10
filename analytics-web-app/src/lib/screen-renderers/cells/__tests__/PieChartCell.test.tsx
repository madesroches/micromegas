import { render, screen, fireEvent } from '@testing-library/react'
import { makeTable, tableFromArrays, vectorFromArray, Utf8 } from 'apache-arrow'
import { PieChartCell, groupPieSlices, buildSliceGeometry, pieChartMetadata } from '../PieChartCell'
import type { ResolvedPieSlice } from '../PieChartCell'
import type { CellRendererProps, CellEditorProps } from '../../cell-registry'
import type { PieSlice } from '@/lib/arrow-utils'
import type { QueryCellConfig } from '../../notebook-types'

const createMockProps = (overrides: Partial<CellRendererProps> = {}): CellRendererProps => ({
  name: 'test-piechart',
  sql: undefined,
  options: undefined,
  data: [],
  status: 'success',
  error: undefined,
  timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
  variables: {},
  isEditing: false,
  onRun: vi.fn(),
  onSqlChange: vi.fn(),
  onOptionsChange: vi.fn(),
  cellResults: {},
  cellSelections: {},
  ...overrides,
})

function pieTable(rows: { label: string; value: number }[]) {
  // `tableFromArrays` dictionary-encodes plain string[] columns by default, but
  // `validateChartColumns` checks the label column's type without unwrapping
  // dictionaries — build an explicit plain-Utf8 vector to match what a real
  // SQL query result looks like.
  return makeTable({
    label: vectorFromArray(
      rows.map((r) => r.label),
      new Utf8(),
    ),
    value: Float64Array.from(rows.map((r) => r.value)),
  })
}

describe('groupPieSlices', () => {
  const slices: PieSlice[] = [
    { label: 'A', value: 100 },
    { label: 'B', value: 80 },
    { label: 'C', value: 60 },
    { label: 'D', value: 40 },
    { label: 'E', value: 20 },
  ]

  it('leaves slices untouched when at or under the cap', () => {
    const resolved = groupPieSlices(slices, 8)
    expect(resolved).toHaveLength(5)
    expect(resolved.every((s) => s.foldedCount === undefined)).toBe(true)
  })

  it('sorts descending by value', () => {
    const shuffled: PieSlice[] = [
      { label: 'small', value: 1 },
      { label: 'big', value: 99 },
    ]
    const resolved = groupPieSlices(shuffled, 8)
    expect(resolved[0].label).toBe('big')
    expect(resolved[1].label).toBe('small')
  })

  it('folds the tail into "Other" at the max_slices boundary', () => {
    // 5 slices, cap of 3: keep top 2, fold remaining 3 into "Other"
    const resolved = groupPieSlices(slices, 3)
    expect(resolved).toHaveLength(3)
    expect(resolved[0].label).toBe('A')
    expect(resolved[1].label).toBe('B')
    expect(resolved[2].label).toBe('Other')
    expect(resolved[2].value).toBe(60 + 40 + 20)
    expect(resolved[2].foldedCount).toBe(3)
  })

  it('gives "Other" the fixed muted color, never the rotating palette', () => {
    const resolved = groupPieSlices(slices, 3)
    expect(resolved[2].color).toBe('var(--text-muted)')
  })

  it('assigns palette colors in fixed order, skipping SQL-supplied colors', () => {
    const withColor: PieSlice[] = [
      { label: 'X', value: 10, color: '#123456ff' },
      { label: 'Y', value: 5 },
    ]
    const resolved = groupPieSlices(withColor, 8)
    expect(resolved[0].color).toBe('#123456ff')
    // Y has no SQL color, so it gets the first palette entry (not the second)
    expect(resolved[1].color).toBe('#bf360c')
  })
})

describe('buildSliceGeometry', () => {
  // A single category holding 100% of the total is a degenerate case for the raw
  // arc math: start/end angles differ by exactly 2π, so their cartesian points
  // coincide and a plain `A` arc command collapses to nothing (blank disc).
  const fullSlice: ResolvedPieSlice[] = [{ label: 'A', value: 10, color: '#123456' }]

  it('produces a non-degenerate full-ring path for a single 100%-share pie slice', () => {
    const [g] = buildSliceGeometry(fullSlice, 10, 'pie')
    expect(g.fraction).toBe(1)
    // Two 180° arcs back-to-back, not a single (degenerate) 360° arc.
    const arcCount = (g.path.match(/A\s/g) ?? []).length
    expect(arcCount).toBe(2)
    // The two arc endpoints must be distinct points, not a coincident start/end.
    const points = [...g.path.matchAll(/A [\d.]+ [\d.]+ 0 1 1 ([\d.-]+) ([\d.-]+)/g)].map(
      (m) => `${m[1]},${m[2]}`,
    )
    expect(new Set(points).size).toBe(points.length)
  })

  it('produces a non-degenerate full-ring path (outer + inner circle) for a single 100%-share donut slice', () => {
    const [g] = buildSliceGeometry(fullSlice, 10, 'donut')
    expect(g.fraction).toBe(1)
    // Outer circle (2 arcs) + inner circle (2 arcs) = 4 arc commands total.
    const arcCount = (g.path.match(/A\s/g) ?? []).length
    expect(arcCount).toBe(4)
    const points = [...g.path.matchAll(/A [\d.]+ [\d.]+ 0 1 1 ([\d.-]+) ([\d.-]+)/g)].map(
      (m) => `${m[1]},${m[2]}`,
    )
    expect(new Set(points).size).toBe(points.length)
  })
})

describe('PieChartCell renderer', () => {
  it('shows a loading indicator when status is loading', () => {
    render(<PieChartCell {...createMockProps({ status: 'loading' })} />)
    expect(screen.getByText('Loading...')).toBeInTheDocument()
  })

  it('shows "No data available" when there is no table', () => {
    render(<PieChartCell {...createMockProps({ data: [] })} />)
    expect(screen.getByText('No data available')).toBeInTheDocument()
  })

  it('shows "No data available" when the table has zero rows', () => {
    const table = pieTable([])
    render(<PieChartCell {...createMockProps({ data: [table] })} />)
    expect(screen.getByText('No data available')).toBeInTheDocument()
  })

  it('shows an error message for an invalid query shape', () => {
    const table = tableFromArrays({ onlyOneColumn: ['a', 'b'] })
    render(<PieChartCell {...createMockProps({ data: [table] })} />)
    expect(screen.getByText(/X and Y columns/)).toBeInTheDocument()
  })

  it('renders the legend, category count, and total on the happy path', () => {
    const table = pieTable([
      { label: 'ERROR', value: 40 },
      { label: 'WARN', value: 10 },
    ])
    // Pie mode avoids the donut center total duplicating the header's total text.
    render(<PieChartCell {...createMockProps({ data: [table], options: { chart_type: 'pie' } })} />)
    expect(screen.getByText('ERROR')).toBeInTheDocument()
    expect(screen.getByText('WARN')).toBeInTheDocument()
    expect(screen.getByText('2')).toBeInTheDocument() // category count
    expect(screen.getByText('50')).toBeInTheDocument() // total
  })

  it('folds extra categories into "Other" when they exceed max_slices', () => {
    const rows = Array.from({ length: 10 }, (_, i) => ({ label: `cat${i}`, value: 10 - i }))
    const table = pieTable(rows)
    render(
      <PieChartCell
        {...createMockProps({ data: [table], options: { max_slices: 3 } })}
      />
    )
    // top 2 kept + 1 "Other" = 3 categories
    expect(screen.getByText('3')).toBeInTheDocument()
    expect(screen.getByText('cat0')).toBeInTheDocument()
    expect(screen.getByText('cat1')).toBeInTheDocument()
    expect(screen.getByText(/Other/)).toBeInTheDocument()
  })

  it('calls onOptionsChange with the new chart_type when the toggle is clicked', () => {
    const table = pieTable([{ label: 'A', value: 1 }])
    const onOptionsChange = vi.fn()
    render(
      <PieChartCell
        {...createMockProps({ data: [table], options: { chart_type: 'donut' }, onOptionsChange })}
      />
    )
    fireEvent.click(screen.getByText('Pie'))
    expect(onOptionsChange).toHaveBeenCalledWith({ chart_type: 'pie' })
  })

  it('shows the donut center total only in donut mode', () => {
    const table = pieTable([{ label: 'A', value: 42 }])
    const { rerender } = render(
      <PieChartCell {...createMockProps({ data: [table], options: { chart_type: 'donut' } })} />
    )
    expect(screen.getByText('total')).toBeInTheDocument()

    rerender(<PieChartCell {...createMockProps({ data: [table], options: { chart_type: 'pie' } })} />)
    expect(screen.queryByText('total')).not.toBeInTheDocument()
  })
})

describe('pieChartMetadata', () => {
  it('creates a default config with donut chart type and the default max_slices', () => {
    const cfg = pieChartMetadata.createDefaultConfig() as QueryCellConfig
    expect(cfg.type).toBe('piechart')
    expect(cfg.options?.chart_type).toBe('donut')
    expect(cfg.options?.max_slices).toBe(8)
    expect(typeof cfg.sql).toBe('string')
  })

  it('executes via runQueryAs when available, passing the cell name and data source', async () => {
    const runQueryAs = vi.fn().mockResolvedValue(pieTable([{ label: 'A', value: 1 }]))
    const runQuery = vi.fn()
    const config = {
      type: 'piechart' as const,
      name: 'my_pie',
      layout: { height: 320 },
      sql: 'SELECT level, count(*) AS count FROM log_entries GROUP BY level',
      dataSource: 'staging',
    }
    const result = await pieChartMetadata.execute!(config, {
      variables: {},
      cellResults: {},
      cellSelections: {},
      timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
      runQuery,
      runQueryAs,
    })
    expect(runQueryAs).toHaveBeenCalledWith(expect.any(String), 'my_pie', 'staging')
    expect(runQuery).not.toHaveBeenCalled()
    expect(result?.data).toHaveLength(1)
  })
})

describe('PieChartCellEditor', () => {
  const createEditorProps = (overrides: Partial<CellEditorProps> = {}): CellEditorProps => ({
    config: {
      type: 'piechart' as const,
      name: 'my_pie',
      layout: { height: 320 },
      sql: 'SELECT level, count(*) AS count FROM log_entries GROUP BY level',
      options: { chart_type: 'donut', max_slices: 8 },
    },
    onChange: vi.fn(),
    variables: {},
    timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
    cellResults: {},
    cellSelections: {},
    ...overrides,
  })

  it('updates the unit option when the Unit field changes', () => {
    const onChange = vi.fn()
    render(<pieChartMetadata.EditorComponent {...createEditorProps({ onChange })} />)
    const unitInput = screen.getByPlaceholderText('e.g., count, bytes, ms')
    fireEvent.change(unitInput, { target: { value: 'ms' } })
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ options: expect.objectContaining({ unit: 'ms' }) })
    )
  })

  it('updates max_slices when the Max Slices field changes', () => {
    const onChange = vi.fn()
    render(<pieChartMetadata.EditorComponent {...createEditorProps({ onChange })} />)
    const maxSlicesInput = screen.getByDisplayValue('8')
    fireEvent.change(maxSlicesInput, { target: { value: '5' } })
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ options: expect.objectContaining({ max_slices: 5 }) })
    )
  })

  it('updates chart_type when the Pie/Donut toggle is clicked', () => {
    const onChange = vi.fn()
    render(<pieChartMetadata.EditorComponent {...createEditorProps({ onChange })} />)
    fireEvent.click(screen.getByRole('button', { name: 'Pie' }))
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ options: expect.objectContaining({ chart_type: 'pie' }) })
    )
  })
})
