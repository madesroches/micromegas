import { render, screen, fireEvent, within } from '@testing-library/react'
import { makeTable, tableFromArrays, vectorFromArray, Utf8 } from 'apache-arrow'
import {
  StackedBarCell,
  buildHorizontalBarLayout,
  buildBarSelection,
  resolveSeriesColors,
  niceStep,
  buildStackedBarLayout,
  nearestSegmentSeries,
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
// effect resolves synchronously to a fixed size.
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
    expect(layout.valueTicks.map((t) => t.value)).toEqual([0, 20, 40, 60, 80])
  })

  it('gives a 100 tick with headroom above it for a 100-total percent series', () => {
    const data = { categories: ['a'], series: [{ name: 'x', color: '#111' }], values: [[100]] }
    const layout = buildStackedBarLayout(data, { width: 400, height: 300, unit: 'percent' })
    const values = layout.valueTicks.map((t) => t.value)
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

  it('floors bar width at 12px and grows contentWidth beyond the container to force horizontal scroll', () => {
    const categories = Array.from({ length: 100 }, (_, i) => `cat${i}`)
    const data = { categories, series: [{ name: 'x', color: '#1' }], values: categories.map(() => [10]) }
    const layout = buildStackedBarLayout(data, { width: 400, height: 300, unit: '' })
    expect(layout.barWidth).toBe(12)
    expect(layout.contentWidth).toBeGreaterThan(400)
  })

  it('rotates x labels when a category label is wider than its band', () => {
    const categories = ['a very very long category label', 'b', 'c']
    const data = { categories, series: [{ name: 'x', color: '#1' }], values: categories.map(() => [10]) }
    const layout = buildStackedBarLayout(data, { width: 200, height: 300, unit: '' })
    expect(layout.rotateLabels).toBe(true)
  })

  it('truncates a long category label with an ellipsis when rotated, keeping the full text for the title', () => {
    const categories = ['a very very long category label that would otherwise run off the plot', 'b', 'c']
    const data = { categories, series: [{ name: 'x', color: '#1' }], values: categories.map(() => [10]) }
    const layout = buildStackedBarLayout(data, { width: 200, height: 300, unit: '' })
    const bar = layout.bars[0]
    expect(layout.rotateLabels).toBe(true)
    expect(bar.displayLabel).not.toBe(bar.category)
    expect(bar.displayLabel.endsWith('…')).toBe(true)
    expect(bar.displayLabel.length).toBeLessThan(bar.category.length)
    expect(bar.category).toBe(categories[0])
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
    expect(screen.getByText(/category and value columns/)).toBeInTheDocument()
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

  it('writes orientation, valueLabels and selectionMode options', () => {
    const onChange = vi.fn()
    render(<stackedBarMetadata.EditorComponent {...createEditorProps({ onChange })} />)
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'horizontal' } })
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ options: expect.objectContaining({ orientation: 'horizontal' }) })
    )
    fireEvent.click(screen.getByLabelText('Show value labels'))
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ options: expect.objectContaining({ valueLabels: true }) })
    )
    fireEvent.click(screen.getByText('Row Selection'))
    fireEvent.click(screen.getByText('Single'))
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ options: expect.objectContaining({ selectionMode: 'single' }) })
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

const resolvedSeries = (names: string[]) => names.map((name, i) => ({ name, color: `#00000${i}` }))

describe('buildStackedBarLayout valueLabels', () => {
  it('grows the top margin and emits total labels when enabled', () => {
    const data = { categories: ['a', 'b'], series: resolvedSeries(['x']), values: [[80], [40]] }
    const off = buildStackedBarLayout(data, { width: 400, height: 300, unit: '' })
    const on = buildStackedBarLayout(data, { width: 400, height: 300, unit: '', valueLabels: true })
    expect(on.topMargin).toBe(off.topMargin + 14)
    expect(off.bars[0].totalLabel).toBeNull()
    expect(on.bars[0].totalLabel?.text).toBe('80')
  })

  it('skips a total label wider than its band', () => {
    const categories = Array.from({ length: 50 }, (_, i) => `c${i}`)
    const data = { categories, series: resolvedSeries(['x']), values: categories.map(() => [123456789]) }
    const layout = buildStackedBarLayout(data, { width: 400, height: 300, unit: '', valueLabels: true })
    expect(layout.bars[0].totalLabel).toBeNull()
  })
})

describe('buildHorizontalBarLayout', () => {
  const rows = (n: number) => Array.from({ length: n }, (_, i) => `user${i}`)

  it('places rows at fixed intervals in first-appearance order', () => {
    const categories = rows(3)
    const data = { categories, series: resolvedSeries(['x']), values: [[30], [20], [10]] }
    const layout = buildHorizontalBarLayout(data, { width: 600, unit: '' })
    expect(layout.orientation).toBe('horizontal')
    expect(layout.bars[1].rowY - layout.bars[0].rowY).toBe(28)
    expect(layout.bars[0].segments[0].width).toBeGreaterThan(layout.bars[2].segments[0].width)
    const taller = buildHorizontalBarLayout(
      { categories: rows(10), series: resolvedSeries(['x']), values: rows(10).map(() => [1]) },
      { width: 600, unit: '' },
    )
    expect(taller.contentHeight).toBeGreaterThan(layout.contentHeight)
    expect(layout.contentWidth).toBe(600)
  })

  it('caps the label column at 180 and truncates long labels', () => {
    const long = 'a very very long category label that will never fit in the column at all'
    const data = { categories: [long, 'b'], series: resolvedSeries(['x']), values: [[2], [1]] }
    const layout = buildHorizontalBarLayout(data, { width: 600, unit: '' })
    expect(layout.labelColumnWidth).toBe(180)
    expect(layout.bars[0].displayLabel.endsWith('…')).toBe(true)
    expect(layout.bars[0].category).toBe(long)
    const short = buildHorizontalBarLayout(
      { categories: ['a'], series: resolvedSeries(['x']), values: [[1]] },
      { width: 600, unit: '' },
    )
    expect(short.labelColumnWidth).toBeLessThan(180)
  })

  it('shrinks the value scale to leave room for total labels', () => {
    const data = { categories: ['a', 'b'], series: resolvedSeries(['x']), values: [[1000], [500]] }
    const off = buildHorizontalBarLayout(data, { width: 600, unit: '' })
    const on = buildHorizontalBarLayout(data, { width: 600, unit: '', valueLabels: true })
    expect(on.bars[0].segments[0].width).toBeLessThan(off.bars[0].segments[0].width)
    expect(on.bars[0].totalLabel?.text).toBe('1,000')
  })

  it('uses fewer tick divisions at narrow widths', () => {
    const data = { categories: ['a'], series: resolvedSeries(['x']), values: [[1000]] }
    const wide = buildHorizontalBarLayout(data, { width: 900, unit: '' })
    const narrow = buildHorizontalBarLayout(data, { width: 160, unit: '' })
    expect(narrow.valueTicks.length).toBeLessThan(wide.valueTicks.length)
  })

  it('stacks segments left to right with a 2px gap and rounds only the end segment', () => {
    const data = { categories: ['a'], series: resolvedSeries(['x', 'y']), values: [[30, 20]] }
    const layout = buildHorizontalBarLayout(data, { width: 600, unit: '' })
    const [first, second] = layout.bars[0].segments
    expect(second.x).toBeCloseTo(first.x + first.width + 2, 5)
    expect([first.rounded, second.rounded]).toEqual([false, true])
  })

  it('checks in-segment label fit against the segment width only', () => {
    const data = { categories: ['a'], series: resolvedSeries(['x', 'y']), values: [[1000, 1]] }
    const layout = buildHorizontalBarLayout(data, { width: 600, unit: '' })
    const [big, tiny] = layout.bars[0].segments
    expect(big.labelFits).toBe(true)
    expect(tiny.labelFits).toBe(false)
  })
})

describe('hitBand', () => {
  it('tiles vertical bands from the y-axis across the full content height', () => {
    const data = { categories: ['a', 'b', 'c'], series: resolvedSeries(['x']), values: [[3], [2], [1]] }
    const layout = buildStackedBarLayout(data, { width: 400, height: 300, unit: '' })
    const [a, b, c] = layout.bars.map((bar) => bar.hitBand)
    expect(a.x).toBe(layout.yAxisWidth)
    expect(b.x).toBeCloseTo(a.x + a.width)
    expect(c.x).toBeCloseTo(b.x + b.width)
    for (const band of [a, b, c]) {
      expect(band.y).toBe(0)
      expect(band.height).toBe(layout.contentHeight)
    }
  })

  it('spans the full row width for horizontal rows', () => {
    const data = { categories: ['a', 'b'], series: resolvedSeries(['x']), values: [[3], [2]] }
    const layout = buildHorizontalBarLayout(data, { width: 600, unit: '' })
    layout.bars.forEach((bar) => {
      expect(bar.hitBand).toEqual({ x: 0, y: bar.rowY, width: layout.contentWidth, height: 28 })
    })
  })
})

describe('nearestSegmentSeries', () => {
  const stackedValues = { categories: ['a', 'b'], series: resolvedSeries(['x', 'y', 'z']), values: [[10, 0, 30], [0, 0, 0]] }

  describe('horizontal', () => {
    const layout = buildHorizontalBarLayout(stackedValues, { width: 600, unit: '' })
    const bar = layout.bars[0]
    const [first, end] = bar.segments

    it('returns the segment containing the position', () => {
      expect(nearestSegmentSeries(bar, 'horizontal', first.x + first.width / 2)).toBe(0)
      expect(nearestSegmentSeries(bar, 'horizontal', end.x + end.width / 2)).toBe(2)
    })
    it('returns the end segment past the bar end', () => {
      expect(nearestSegmentSeries(bar, 'horizontal', end.x + end.width + 50)).toBe(2)
    })
    it('returns the first segment before the baseline', () => {
      expect(nearestSegmentSeries(bar, 'horizontal', 0)).toBe(0)
    })
    it('picks the nearer segment in the gap between segments', () => {
      expect(nearestSegmentSeries(bar, 'horizontal', first.x + first.width + 0.5)).toBe(0)
    })
    it('returns null without segments', () => {
      expect(nearestSegmentSeries(layout.bars[1], 'horizontal', 10)).toBeNull()
    })
  })

  describe('vertical', () => {
    const layout = buildStackedBarLayout(stackedValues, { width: 400, height: 300, unit: '' })
    const bar = layout.bars[0]
    const [first, end] = bar.segments

    it('returns the segment containing the position', () => {
      expect(nearestSegmentSeries(bar, 'vertical', first.y + first.height / 2)).toBe(0)
      expect(nearestSegmentSeries(bar, 'vertical', end.y + end.height / 2)).toBe(2)
    })
    it('returns the end segment above the bar', () => {
      expect(nearestSegmentSeries(bar, 'vertical', 0)).toBe(2)
    })
    it('returns the first segment in the label area below the baseline', () => {
      expect(nearestSegmentSeries(bar, 'vertical', layout.contentHeight - 8)).toBe(0)
    })
    it('picks the nearer segment in the gap between segments', () => {
      expect(nearestSegmentSeries(bar, 'vertical', first.y - 0.5)).toBe(0)
    })
    it('returns null without segments', () => {
      expect(nearestSegmentSeries(layout.bars[1], 'vertical', 10)).toBeNull()
    })
  })
})

describe('buildBarSelection', () => {
  it('keys the payload by SQL column names with the raw category and summed value', () => {
    const base = { categoryValues: [42], series: [{ name: 'm' }], values: [[7]] }
    expect(buildBarSelection({ ...base, columns: { category: 'user', series: 'model', value: 'cost' } }, { categoryIndex: 0, seriesIndex: 0 })).toEqual({
      user: 42,
      model: 'm',
      cost: 7,
    })
    expect(buildBarSelection({ ...base, columns: { category: 'user', value: 'cost' } }, { categoryIndex: 0, seriesIndex: 0 })).toEqual({
      user: 42,
      cost: 7,
    })
  })
})

function singleTable(rows: { user: string; cost: number }[]) {
  return makeTable({
    user: vectorFromArray(rows.map((r) => r.user), new Utf8()),
    cost: Float64Array.from(rows.map((r) => r.cost)),
  })
}

describe('StackedBarCell selection and orientation', () => {
  const stacked = () =>
    stackedBarTable([
      { category: 'a', series: 'first', value: 10 },
      { category: 'a', series: 'second', value: 30 },
      { category: 'b', series: 'first', value: 5 },
    ])

  it('publishes the clicked segment, dims the others, and toggles off on a second click', () => {
    const onSelectionChange = vi.fn()
    const props = createMockProps({ data: [stacked()], selectionMode: 'single', onSelectionChange })
    const { container } = render(<StackedBarCell {...props} />)
    onSelectionChange.mockClear()
    const paths = () => container.querySelectorAll('svg path')
    fireEvent.click(paths()[1])
    expect(onSelectionChange).toHaveBeenLastCalledWith({ category: 'a', series: 'second', value: 30 })
    expect((paths()[0] as SVGElement).style.opacity).toBe('0.25')
    expect((paths()[1] as SVGElement).style.opacity).toBe('')
    fireEvent.click(paths()[1])
    expect(onSelectionChange).toHaveBeenLastCalledWith(null)
    expect((paths()[0] as SVGElement).style.opacity).toBe('')
  })

  it('clears the selection on a background click', () => {
    const onSelectionChange = vi.fn()
    const props = createMockProps({ data: [stacked()], selectionMode: 'single', onSelectionChange })
    const { container } = render(<StackedBarCell {...props} />)
    fireEvent.click(container.querySelectorAll('svg path')[0])
    fireEvent.click(container.querySelector('svg') as SVGElement)
    expect(onSelectionChange).toHaveBeenLastCalledWith(null)
    expect((container.querySelectorAll('svg path')[1] as SVGElement).style.opacity).toBe('')
  })

  it('clears the selection and notifies when data changes', () => {
    const onSelectionChange = vi.fn()
    const props = createMockProps({ data: [stacked()], selectionMode: 'single', onSelectionChange })
    const { container, rerender } = render(<StackedBarCell {...props} />)
    fireEvent.click(container.querySelectorAll('svg path')[0])
    onSelectionChange.mockClear()
    rerender(<StackedBarCell {...props} data={[stacked()]} />)
    expect(onSelectionChange).toHaveBeenCalledWith(null)
    expect((container.querySelectorAll('svg path')[1] as SVGElement).style.opacity).toBe('')
  })

  it('ignores clicks when selectionMode is unset', () => {
    const onSelectionChange = vi.fn()
    const props = createMockProps({ data: [stacked()], onSelectionChange })
    const { container } = render(<StackedBarCell {...props} />)
    onSelectionChange.mockClear()
    fireEvent.click(container.querySelectorAll('svg path')[0])
    expect(onSelectionChange).not.toHaveBeenCalled()
  })

  it('renders a single-series chart without legend, series stat, or share rows', () => {
    const table = singleTable([
      { user: 'a', cost: 10 },
      { user: 'b', cost: 5 },
    ])
    const { container } = render(<StackedBarCell {...createMockProps({ data: [table] })} />)
    expect(screen.queryByText('series:')).not.toBeInTheDocument()
    expect(container.querySelectorAll('span[title]')).toHaveLength(0)
    fireEvent.pointerMove(container.querySelectorAll('svg path')[0], { clientX: 5, clientY: 5 })
    expect(screen.getByText('cost')).toBeInTheDocument()
    expect(screen.queryByText('share of bar')).not.toBeInTheDocument()
    expect(screen.queryByText('bar total')).not.toBeInTheDocument()
  })

  it('publishes only category and value for the 2-column shape', () => {
    const onSelectionChange = vi.fn()
    const table = singleTable([{ user: 'a', cost: 10 }])
    const { container } = render(
      <StackedBarCell {...createMockProps({ data: [table], selectionMode: 'single', onSelectionChange })} />,
    )
    fireEvent.click(container.querySelector('svg path') as SVGElement)
    expect(onSelectionChange).toHaveBeenLastCalledWith({ user: 'a', cost: 10 })
  })

  it('renders a sticky axis svg plus a body svg in horizontal mode, end-anchoring the last tick', () => {
    const table = singleTable([
      { user: 'a', cost: 10 },
      { user: 'b', cost: 5 },
    ])
    render(<StackedBarCell {...createMockProps({ data: [table], options: { orientation: 'horizontal' } })} />)
    const axis = screen.getByTestId('bar-axis')
    const body = screen.getByTestId('bar-body')
    expect(body.getAttribute('height')).toBe(String(2 * 28 + 8))
    const anchors = Array.from(axis.querySelectorAll('text')).map((t) => t.getAttribute('text-anchor'))
    expect(anchors[anchors.length - 1]).toBe('end')
    expect(anchors.slice(0, -1).every((a) => a === 'middle')).toBe(true)
  })

  describe('band selection', () => {
    const horizontal = { orientation: 'horizontal' }
    const hitRects = (container: HTMLElement) => container.querySelectorAll('rect[data-bar-hit]')

    it('selects a row from its hit rect and clears on a second click (2-column shape)', () => {
      const onSelectionChange = vi.fn()
      const table = singleTable([
        { user: 'a', cost: 10 },
        { user: 'b', cost: 5 },
      ])
      const { container } = render(
        <StackedBarCell
          {...createMockProps({ data: [table], options: horizontal, selectionMode: 'single', onSelectionChange })}
        />,
      )
      const rects = hitRects(container)
      expect(rects).toHaveLength(2)
      fireEvent.click(rects[1])
      expect(onSelectionChange).toHaveBeenLastCalledWith({ user: 'b', cost: 5 })
      fireEvent.click(hitRects(container)[1])
      expect(onSelectionChange).toHaveBeenLastCalledWith(null)
    })

    it('selects the baseline segment from a category label', () => {
      const onSelectionChange = vi.fn()
      const { container } = render(
        <StackedBarCell
          {...createMockProps({ data: [stacked()], options: horizontal, selectionMode: 'single', onSelectionChange })}
        />,
      )
      const label = Array.from(container.querySelectorAll('text[data-bar-hit]')).find((t) => t.querySelector('title')?.textContent === 'a')
      fireEvent.click(label as Element)
      expect(onSelectionChange).toHaveBeenLastCalledWith({ category: 'a', series: 'first', value: 10 })
    })

    it('selects the end segment from a band click past the bar end', () => {
      const onSelectionChange = vi.fn()
      const { container } = render(
        <StackedBarCell
          {...createMockProps({ data: [stacked()], options: horizontal, selectionMode: 'single', onSelectionChange })}
        />,
      )
      fireEvent.click(hitRects(container)[0], { clientX: 595 })
      expect(onSelectionChange).toHaveBeenLastCalledWith({ category: 'a', series: 'second', value: 30 })
    })

    it('picks the nearest segment from clientY, not clientX, in vertical orientation', () => {
      const onSelectionChange = vi.fn()
      const { container } = render(
        <StackedBarCell {...createMockProps({ data: [stacked()], selectionMode: 'single', onSelectionChange })} />,
      )
      fireEvent.click(hitRects(container)[0], { clientX: 0, clientY: 0 })
      expect(onSelectionChange).toHaveBeenLastCalledWith({ category: 'a', series: 'second', value: 30 })
      fireEvent.click(hitRects(container)[0], { clientX: 500, clientY: 250 })
      expect(onSelectionChange).toHaveBeenLastCalledWith({ category: 'a', series: 'first', value: 10 })
    })

    it('clears the selection when the axis svg is clicked', () => {
      const onSelectionChange = vi.fn()
      const { container } = render(
        <StackedBarCell
          {...createMockProps({ data: [stacked()], options: horizontal, selectionMode: 'single', onSelectionChange })}
        />,
      )
      fireEvent.click(container.querySelector('svg path') as SVGElement)
      fireEvent.click(screen.getByTestId('bar-axis'))
      expect(onSelectionChange).toHaveBeenLastCalledWith(null)
    })

    it('converts client coordinates using the plot svg rect', () => {
      const onSelectionChange = vi.fn()
      const layout = buildHorizontalBarLayout(
        { categories: ['a', 'b'], series: resolvedSeries(['first', 'second']), values: [[10, 30], [5, 0]] },
        { width: 600, unit: '' },
      )
      const first = layout.bars[0].segments[0]
      const { container } = render(
        <StackedBarCell
          {...createMockProps({ data: [stacked()], options: horizontal, selectionMode: 'single', onSelectionChange })}
        />,
      )
      vi.spyOn(screen.getByTestId('bar-body'), 'getBoundingClientRect').mockReturnValue({
        left: 100,
        top: 50,
      } as DOMRect)
      fireEvent.click(hitRects(container)[0], { clientX: 100 + 595 })
      expect(onSelectionChange).toHaveBeenLastCalledWith({ category: 'a', series: 'second', value: 30 })
      fireEvent.click(hitRects(container)[0], { clientX: 100 + first.x + first.width / 2 })
      expect(onSelectionChange).toHaveBeenLastCalledWith({ category: 'a', series: 'first', value: 10 })
    })

    it('renders no hit rects when selectionMode is unset', () => {
      const { container } = render(<StackedBarCell {...createMockProps({ data: [stacked()], options: horizontal })} />)
      expect(container.querySelectorAll('rect[data-bar-hit], text[data-bar-hit]')).toHaveLength(0)
    })
  })

  it('renders total labels when valueLabels is set', () => {
    const table = singleTable([{ user: 'a', cost: 1000 }])
    const { container, rerender } = render(<StackedBarCell {...createMockProps({ data: [table] })} />)
    const before = container.querySelectorAll('svg text').length
    rerender(<StackedBarCell {...createMockProps({ data: [table], options: { valueLabels: true } })} />)
    expect(container.querySelectorAll('svg text').length).toBe(before + 1)
  })
})

describe('stackedBarMetadata label and default names', () => {
  it('is labelled Bar Chart', () => {
    expect(stackedBarMetadata.label).toBe('Bar Chart')
  })

  it('names new cells Bar_Chart, then Bar_Chart_2', async () => {
    // cell-registry pulls in uPlot, which needs matchMedia at import time.
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query) => ({
        matches: false,
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    })
    const { createDefaultCell } = await import('../../cell-registry')
    expect(createDefaultCell('stackedbar', new Set()).name).toBe('Bar_Chart')
    expect(createDefaultCell('stackedbar', new Set(['Bar_Chart'])).name).toBe('Bar_Chart_2')
  })
})
