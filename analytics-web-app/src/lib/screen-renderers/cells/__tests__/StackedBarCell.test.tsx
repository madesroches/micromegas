import { render, screen, fireEvent, within } from '@testing-library/react'
import { makeTable, tableFromArrays, vectorFromArray, Utf8 } from 'apache-arrow'
import {
  StackedBarCell,
  resolveSeriesColors,
  niceStep,
  buildStackedBarLayout,
  stackedBarMetadata,
} from '../StackedBarCell'
import { SERIES_COLORS } from '@/components/chart-constants'
import type { CellRendererProps, CellEditorProps } from '../../cell-registry'
import type { QueryCellConfig } from '../../notebook-types'

const createMockProps = (overrides: Partial<CellRendererProps> = {}): CellRendererProps => ({
  name: 'test-stackedbar',
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

function stackedBarTable(rows: { category: string; series: string; value: number }[]) {
  // Explicit plain-Utf8 vectors, matching what a real SQL query result looks like
  // (see PieChartCell.test.tsx's pieTable — validateChartColumns-family functions
  // don't unwrap dictionaries for this check).
  return makeTable({
    category: vectorFromArray(rows.map((r) => r.category), new Utf8()),
    series: vectorFromArray(rows.map((r) => r.series), new Utf8()),
    value: Float64Array.from(rows.map((r) => r.value)),
  })
}

// jsdom has no ResizeObserver; stub it so the cell's plot-container measurement
// effect resolves synchronously to a fixed size, as the plan's Phase 3 step 7 asks.
class MockResizeObserver {
  callback: ResizeObserverCallback
  constructor(callback: ResizeObserverCallback) {
    this.callback = callback
  }
  observe(target: Element) {
    this.callback(
      [{ target, contentRect: { width: 600, height: 260 } } as ResizeObserverEntry],
      this as unknown as ResizeObserver,
    )
  }
  unobserve() {}
  disconnect() {}
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', MockResizeObserver)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('resolveSeriesColors', () => {
  it('assigns palette colors in series order when no SQL color is given', () => {
    const resolved = resolveSeriesColors([{ name: 'a' }, { name: 'b' }])
    expect(resolved[0].color).toBe(SERIES_COLORS[0])
    expect(resolved[1].color).toBe(SERIES_COLORS[1])
  })

  it('keeps a SQL-supplied color and does not consume a palette slot for it', () => {
    const resolved = resolveSeriesColors([
      { name: 'a', color: '#123456ff' },
      { name: 'b' },
    ])
    expect(resolved[0].color).toBe('#123456ff')
    // 'b' gets the first palette entry, not the second — the SQL color didn't
    // advance the palette index.
    expect(resolved[1].color).toBe(SERIES_COLORS[0])
  })

  it('wraps the palette after 12 series', () => {
    const series = Array.from({ length: 13 }, (_, i) => ({ name: `s${i}` }))
    const resolved = resolveSeriesColors(series)
    expect(resolved[12].color).toBe(resolved[0].color)
  })
})

describe('niceStep', () => {
  it('picks the smallest of {1, 2, 2.5, 5} x 10^n that is >= the input', () => {
    expect(niceStep(0.9)).toBe(1)
    expect(niceStep(1.5)).toBe(2)
    expect(niceStep(2.1)).toBe(2.5)
    expect(niceStep(4)).toBe(5)
    expect(niceStep(7)).toBe(10)
    expect(niceStep(21)).toBe(25)
  })
})

describe('buildStackedBarLayout', () => {
  it('produces clean y-axis tick steps below a headroom top', () => {
    const data = { categories: ['a', 'b'], series: [{ name: 'x', color: '#111' }], values: [[80], [40]] }
    const layout = buildStackedBarLayout(data, { width: 400, height: 300, unit: '' })
    expect(layout.yTicks.map((t) => t.value)).toEqual([0, 20, 40, 60, 80])
  })

  it('gives a 100 tick with headroom above it for a 100-total percent series', () => {
    const data = { categories: ['a'], series: [{ name: 'x', color: '#111' }], values: [[100]] }
    const layout = buildStackedBarLayout(data, { width: 400, height: 300, unit: 'percent' })
    const values = layout.yTicks.map((t) => t.value)
    expect(Math.max(...values)).toBe(100)
    // top = 105 > 100, so the segment reaching 100 doesn't touch the plot's top margin
    const seg = layout.bars[0].segments[0]
    expect(seg.y).toBeGreaterThan(10)
  })

  it('stacks segments contiguously with a 2px gap between them', () => {
    const data = {
      categories: ['a'],
      series: [
        { name: 'x', color: '#111' },
        { name: 'y', color: '#222' },
      ],
      values: [[30, 20]],
    }
    const layout = buildStackedBarLayout(data, { width: 400, height: 300, unit: '' })
    const [bottom, top] = layout.bars[0].segments
    expect(bottom.y).toBeCloseTo(top.y + top.height + 2, 5)
  })

  it('marks only the topmost present segment as rounded, including when the last series is absent from a bar', () => {
    const data = {
      categories: ['a', 'b'],
      series: [
        { name: 'x', color: '#1' },
        { name: 'y', color: '#2' },
        { name: 'z', color: '#3' },
      ],
      values: [
        [10, 10, 10],
        [10, 10, 0], // bar 'b' has no 'z'
      ],
    }
    const layout = buildStackedBarLayout(data, { width: 400, height: 300, unit: '' })
    expect(layout.bars[0].segments.map((s) => s.rounded)).toEqual([false, false, true])
    expect(layout.bars[1].segments).toHaveLength(2)
    expect(layout.bars[1].segments.map((s) => s.rounded)).toEqual([false, true])
  })

  it('floors bar width at 12px and grows plotWidth beyond the container to force horizontal scroll', () => {
    const categories = Array.from({ length: 100 }, (_, i) => `cat${i}`)
    const data = { categories, series: [{ name: 'x', color: '#1' }], values: categories.map(() => [10]) }
    const layout = buildStackedBarLayout(data, { width: 400, height: 300, unit: '' })
    expect(layout.barWidth).toBe(12)
    expect(layout.plotWidth).toBeGreaterThan(400)
  })

  it('rotates x labels when a category label is wider than its band', () => {
    const categories = ['a very very long category label', 'b', 'c']
    const data = { categories, series: [{ name: 'x', color: '#1' }], values: categories.map(() => [10]) }
    const layout = buildStackedBarLayout(data, { width: 200, height: 300, unit: '' })
    expect(layout.rotateLabels).toBe(true)
  })

  it('does not rotate labels that fit within their band', () => {
    const categories = ['a', 'b']
    const data = { categories, series: [{ name: 'x', color: '#1' }], values: categories.map(() => [10]) }
    const layout = buildStackedBarLayout(data, { width: 800, height: 300, unit: '' })
    expect(layout.rotateLabels).toBe(false)
  })

  it('skips the in-segment label when the segment is shorter than 18px', () => {
    const data = {
      categories: ['a'],
      series: [
        { name: 'x', color: '#1' },
        { name: 'y', color: '#2' },
      ],
      values: [[1000, 1]],
    }
    const layout = buildStackedBarLayout(data, { width: 400, height: 300, unit: '' })
    const tiny = layout.bars[0].segments.find((s) => s.value === 1)!
    expect(tiny.labelFits).toBe(false)
  })

  it('skips the in-segment label when the bar is narrower than the label text', () => {
    const categories = Array.from({ length: 50 }, (_, i) => `c${i}`)
    const data = { categories, series: [{ name: 'x', color: '#1' }], values: categories.map(() => [123456789]) }
    const layout = buildStackedBarLayout(data, { width: 400, height: 300, unit: '' })
    expect(layout.barWidth).toBeLessThan(30)
    expect(layout.bars[0].segments[0].labelFits).toBe(false)
  })
})

describe('StackedBarCell renderer', () => {
  it('shows a loading indicator when status is loading', () => {
    render(<StackedBarCell {...createMockProps({ status: 'loading' })} />)
    expect(screen.getByText('Loading...')).toBeInTheDocument()
  })

  it('renders bars once data arrives after mounting in the loading state', () => {
    // Regression test: the plot's ResizeObserver effect used to run with `[]`
    // deps on the top-level cell, so it never re-attached after the cell
    // mounted idle/loading and later transitioned to success — see the plot
    // now living in a child component mounted only on the success path.
    const table = stackedBarTable([{ category: 'a', series: 'first', value: 10 }])
    const { rerender } = render(<StackedBarCell {...createMockProps({ status: 'loading', data: [] })} />)
    expect(screen.getByText('Loading...')).toBeInTheDocument()

    rerender(<StackedBarCell {...createMockProps({ status: 'success', data: [table] })} />)

    expect(screen.queryByText('Loading...')).not.toBeInTheDocument()
    expect(document.querySelectorAll('svg path')).toHaveLength(1)
  })

  it('shows "No data available" when there is no table', () => {
    render(<StackedBarCell {...createMockProps({ data: [] })} />)
    expect(screen.getByText('No data available')).toBeInTheDocument()
  })

  it('shows "No data available" when the table has zero rows', () => {
    const table = stackedBarTable([])
    render(<StackedBarCell {...createMockProps({ data: [table] })} />)
    expect(screen.getByText('No data available')).toBeInTheDocument()
  })

  it('shows an error message for an invalid query shape', () => {
    const table = tableFromArrays({ onlyOneColumn: ['a', 'b'] })
    render(<StackedBarCell {...createMockProps({ data: [table] })} />)
    expect(screen.getByText(/category, series, and value columns/)).toBeInTheDocument()
  })

  it('shows "No data available" when every value is 0, instead of a blank plot', () => {
    const table = stackedBarTable([
      { category: 'a', series: 'x', value: 0 },
      { category: 'b', series: 'x', value: 0 },
    ])
    render(<StackedBarCell {...createMockProps({ data: [table] })} />)
    expect(screen.getByText('No data available')).toBeInTheDocument()
  })

  it('renders header stats and legend rows in stack (first-appearance) order', () => {
    const table = stackedBarTable([
      { category: 'a', series: 'first', value: 10 },
      { category: 'a', series: 'second', value: 5 },
      { category: 'a', series: 'third', value: 2 },
      { category: 'b', series: 'first', value: 3 },
    ])
    const { container } = render(<StackedBarCell {...createMockProps({ data: [table] })} />)

    const header = container.querySelector('.border-b') as HTMLElement
    expect(within(header).getByText('categories:').parentElement?.textContent).toContain('2')
    expect(within(header).getByText('series:').parentElement?.textContent).toContain('3')

    const legendTitles = Array.from(container.querySelectorAll('span[title]')).map((el) => el.getAttribute('title'))
    expect(legendTitles).toEqual(['first', 'second', 'third'])
  })

  it('shows tooltip content, including share of bar, on pointerMove', () => {
    const table = stackedBarTable([
      { category: 'a', series: 'first', value: 10 },
      { category: 'a', series: 'second', value: 30 },
    ])
    const { container } = render(<StackedBarCell {...createMockProps({ data: [table] })} />)

    const paths = container.querySelectorAll('svg path')
    expect(paths).toHaveLength(2)
    fireEvent.pointerMove(paths[0], { clientX: 50, clientY: 50 })

    expect(screen.getByText('share of bar')).toBeInTheDocument()
    expect(screen.getByText('25.0%')).toBeInTheDocument() // 10 / (10 + 30)
    expect(screen.getByText('bar total')).toBeInTheDocument()
  })
})

describe('stackedBarMetadata', () => {
  it('creates a default config with the default SQL and no options', () => {
    const cfg = stackedBarMetadata.createDefaultConfig() as QueryCellConfig
    expect(cfg.type).toBe('stackedbar')
    expect(typeof cfg.sql).toBe('string')
  })

  it('executes via runQueryAs when available, passing the cell name and data source', async () => {
    const runQueryAs = vi.fn().mockResolvedValue(stackedBarTable([{ category: 'a', series: 'x', value: 1 }]))
    const runQuery = vi.fn()
    const config = {
      type: 'stackedbar' as const,
      name: 'my_bar',
      layout: { height: 360 },
      sql: 'SELECT category, series, count(*) AS value FROM log_entries GROUP BY 1, 2',
      dataSource: 'staging',
    }
    const result = await stackedBarMetadata.execute!(config, {
      variables: {},
      cellResults: {},
      cellSelections: {},
      timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
      runQuery,
      runQueryAs,
    })
    expect(runQueryAs).toHaveBeenCalledWith(expect.any(String), 'my_bar', 'staging')
    expect(runQuery).not.toHaveBeenCalled()
    expect(result?.data).toHaveLength(1)
  })
})

describe('StackedBarCellEditor', () => {
  const createEditorProps = (overrides: Partial<CellEditorProps> = {}): CellEditorProps => ({
    config: {
      type: 'stackedbar' as const,
      name: 'my_bar',
      layout: { height: 360 },
      sql: 'SELECT category, series, value FROM t',
      options: {},
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
    render(<stackedBarMetadata.EditorComponent {...createEditorProps({ onChange })} />)
    const unitInput = screen.getByPlaceholderText('e.g., count, bytes, ms, percent')
    fireEvent.change(unitInput, { target: { value: 'percent' } })
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ options: expect.objectContaining({ unit: 'percent' }) })
    )
  })

  it('shows a macro validation error for an unresolvable unit macro', () => {
    const props = createEditorProps({
      config: {
        type: 'stackedbar' as const,
        name: 'my_bar',
        layout: { height: 360 },
        sql: 'SELECT category, series, value FROM t',
        options: { unit: '$missingcell[0].col' },
      },
    })
    render(<stackedBarMetadata.EditorComponent {...props} />)
    expect(screen.getByText(/Unit: Unknown cell/)).toBeInTheDocument()
  })
})
