import { act, render, screen, fireEvent } from '@testing-library/react'
import { tableFromArrays } from 'apache-arrow'
import { ChartCell, chartMetadata } from '../ChartCell'
import type { CellRendererProps, CellEditorProps } from '../../cell-registry'
import type { ChartPointHit } from '@/components/xychart-selection'

interface CapturedProps {
  onPointSelect?: (hit: ChartPointHit | null) => void
  selectedPoint?: ChartPointHit | null
}
let captured: CapturedProps = {}

vi.mock('@/components/XYChart', () => ({
  XYChart: (props: CapturedProps) => {
    captured = props
    return <div data-testid="xychart" />
  },
}))

vi.mock('@/lib/data-sources-api', () => ({
  getDataSourceList: vi.fn().mockReturnValue(new Promise(() => {})),
}))

const makeData = (n = 3, offset = 0) =>
  tableFromArrays({
    cat: Array.from({ length: n }, (_, i) => `c${i}`),
    val: Float64Array.from({ length: n }, (_, i) => offset + i + 1),
  })

const createProps = (overrides: Partial<CellRendererProps> = {}): CellRendererProps => ({
  name: 'chart',
  sql: undefined,
  options: undefined,
  data: [makeData()],
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

describe('ChartCell selection', () => {
  beforeEach(() => {
    captured = {}
  })

  it('is off without selectionMode', () => {
    render(<ChartCell {...createProps()} />)
    expect(captured.onPointSelect).toBeUndefined()
  })

  it('publishes the source row and passes the selection back', () => {
    const onSelectionChange = vi.fn()
    const data = [makeData()]
    const { rerender } = render(
      <ChartCell {...createProps({ data, selectionMode: 'single', onSelectionChange })} />
    )
    onSelectionChange.mockClear()
    act(() => captured.onPointSelect!({ seriesIndex: 0, rowIndex: 2 }))
    expect(onSelectionChange).toHaveBeenLastCalledWith({ cat: 'c2', val: 3 })
    expect(captured.selectedPoint).toEqual({ seriesIndex: 0, rowIndex: 2 })

    rerender(<ChartCell {...createProps({ data, selectionMode: undefined, onSelectionChange })} />)
    expect(captured.selectedPoint).toBeNull()
  })

  it('publishes from the table of the selected series', () => {
    const onSelectionChange = vi.fn()
    const data = [makeData(3, 0), makeData(3, 100)]
    render(<ChartCell {...createProps({ data, selectionMode: 'single', onSelectionChange })} />)
    onSelectionChange.mockClear()
    act(() => captured.onPointSelect!({ seriesIndex: 1, rowIndex: 1 }))
    expect(onSelectionChange).toHaveBeenLastCalledWith({ cat: 'c1', val: 102 })
  })

  it('publishes null when the selection is cleared', () => {
    const onSelectionChange = vi.fn()
    render(<ChartCell {...createProps({ selectionMode: 'single', onSelectionChange })} />)
    act(() => captured.onPointSelect!({ seriesIndex: 0, rowIndex: 0 }))
    onSelectionChange.mockClear()
    act(() => captured.onPointSelect!(null))
    expect(onSelectionChange).toHaveBeenLastCalledWith(null)
    expect(captured.selectedPoint).toBeNull()
  })

  it('clears state and published selection when data changes', () => {
    const onSelectionChange = vi.fn()
    const { rerender } = render(
      <ChartCell {...createProps({ data: [makeData()], selectionMode: 'single', onSelectionChange })} />
    )
    act(() => captured.onPointSelect!({ seriesIndex: 0, rowIndex: 1 }))
    onSelectionChange.mockClear()
    rerender(<ChartCell {...createProps({ data: [makeData()], selectionMode: 'single', onSelectionChange })} />)
    expect(captured.selectedPoint).toBeNull()
    expect(onSelectionChange).toHaveBeenCalledWith(null)
  })
})

describe('ChartCellEditor selection', () => {
  it('writes selectionMode into a v2 config when migrating a v1 config', () => {
    const onChange = vi.fn()
    const props: CellEditorProps = {
      config: { type: 'chart', name: 'chart', layout: { height: 300 }, sql: 'SELECT 1' } as never,
      onChange,
      variables: {},
      timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
      cellResults: {},
      cellSelections: {},
    }
    render(<chartMetadata.EditorComponent {...props} />)
    fireEvent.click(screen.getByText('Row Selection'))
    fireEvent.click(screen.getByLabelText('Single'))
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        version: 2,
        options: expect.objectContaining({ selectionMode: 'single' }),
      })
    )
  })
})
