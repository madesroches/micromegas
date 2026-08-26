import { render, screen } from '@testing-library/react'
import { TransposedTableCell } from '../TransposedTableCell'
import type { CellRendererProps } from '../../cell-registry'
import type { ColumnOverride } from '../../table-utils'
import { makeHistogramTable, SAMPLE_HISTOGRAM_ROW } from '../../__tests__/histogram-fixtures'

const createMockProps = (overrides: Partial<CellRendererProps> = {}): CellRendererProps => ({
  name: 'test-transposed',
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

// =============================================================================
// TransposedTableCell — histogram row render-mode switch (Design §3)
// =============================================================================

describe('TransposedTableCell — histogram row rendering', () => {
  it('renders HistogramCell by default for a histogram-typed row (no override)', () => {
    const table = makeHistogramTable([{ name: 'a', dist: SAMPLE_HISTOGRAM_ROW }])
    const { container } = render(
      <TransposedTableCell {...createMockProps({ data: [table] })} />
    )
    const rects = container.querySelectorAll('svg[data-testid="histogram-track"] rect')
    expect(rects.length).toBe(SAMPLE_HISTOGRAM_ROW.bins.length)
  })

  it('renders HistogramCell with the resolved histogramColor when kind is "histogram"', () => {
    const table = makeHistogramTable([{ name: 'a', dist: SAMPLE_HISTOGRAM_ROW }])
    const overrides: ColumnOverride[] = [{ column: 'dist', kind: 'histogram', histogramColor: '#123456' }]
    const { container } = render(
      <TransposedTableCell {...createMockProps({ data: [table], options: { overrides } })} />
    )
    const rects = container.querySelectorAll('svg[data-testid="histogram-track"] rect')
    expect(rects.length).toBeGreaterThan(0)
    for (const rect of Array.from(rects)) {
      expect(rect.getAttribute('fill')).toBe('#123456')
    }
  })

  it('renders OverrideCell (markdown) when kind is "markdown", even on a histogram row', () => {
    const table = makeHistogramTable([{ name: 'a', dist: SAMPLE_HISTOGRAM_ROW }])
    const overrides: ColumnOverride[] = [{ column: 'dist', format: '$row.dist' }]
    const { container } = render(
      <TransposedTableCell {...createMockProps({ data: [table], options: { overrides } })} />
    )
    // No histogram bars — the row rendered as a Markdown override instead.
    expect(container.querySelector('svg[data-testid="histogram-track"]')).not.toBeInTheDocument()
    // formatArrowValue's histogram branch: a compact, readable field dump.
    expect(screen.getByText('{start:0, end:50, count:40, bins:[1,3,6,10,8,6,3,2,1,0]}')).toBeInTheDocument()
  })

  it('renders "-" for a null histogram value', () => {
    const table = makeHistogramTable([{ name: 'a', dist: null }])
    render(<TransposedTableCell {...createMockProps({ data: [table] })} />)
    expect(screen.getByText('-')).toBeInTheDocument()
  })
})
