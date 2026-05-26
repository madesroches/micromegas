import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import type { DataType } from 'apache-arrow'
import { Table, Timestamp, TimeUnit, vectorFromArray } from 'apache-arrow'
import { EventDetailPanel } from '../EventDetailPanel'
import { columnTypeMap, rowValues } from '../overlay'

function buildRow(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return { x: '1', y: '2', z: '3', ...overrides }
}

function renderPanel(props: {
  row?: Record<string, unknown>
  columnTypes?: Map<string, DataType>
  template: string
  variables?: Record<string, string | Record<string, string>>
  timeRange?: { begin: string; end: string }
}) {
  return render(
    <MemoryRouter>
      <EventDetailPanel
        row={props.row ?? buildRow()}
        columnTypes={props.columnTypes ?? new Map()}
        template={props.template}
        variables={props.variables ?? {}}
        timeRange={props.timeRange ?? { begin: '2026-01-01T00:00:00Z', end: '2026-01-02T00:00:00Z' }}
        cellResults={{}}
        cellSelections={{}}
        onClose={jest.fn()}
      />
    </MemoryRouter>,
  )
}

describe('EventDetailPanel', () => {
  it('substitutes $x/$y/$z column macros', () => {
    renderPanel({
      template: 'Location: ($x, $y, $z)',
      row: buildRow({ x: '10.5', y: '-3', z: '7' }),
    })
    expect(screen.getByText('Location: (10.5, -3, 7)')).toBeInTheDocument()
  })

  it('substitutes $time when the row provides it', () => {
    renderPanel({
      template: 'At $time',
      row: buildRow({ x: '0', y: '0', z: '0', time: '2026-05-01T12:00:00.000Z' }),
    })
    expect(screen.getByText('At 2026-05-01T12:00:00.000Z')).toBeInTheDocument()
  })

  it('renders a Timestamp column as RFC3339 from a bare $col macro', () => {
    const timestampType = new Timestamp(TimeUnit.MILLISECOND, null)
    const table = new Table({
      time: vectorFromArray([1705314600000], timestampType),
      x: vectorFromArray([0]),
      y: vectorFromArray([0]),
      z: vectorFromArray([0]),
    })
    renderPanel({
      template: 'At $time',
      row: rowValues(table, 0),
      columnTypes: columnTypeMap(table),
    })
    expect(screen.getByText('At 2024-01-15T10:30:00.000Z')).toBeInTheDocument()
  })

  it('feeds format_value() the raw column value (precision-preserving)', () => {
    renderPanel({
      template: 'Size: format_value($size, "bytes")',
      // BigInt straight off an Int64 column — never stringified before the
      // template sees it.
      row: buildRow({ x: '0', y: '0', z: '0', size: 3145728n }),
    })
    expect(screen.getByText('Size: 3.0 MB')).toBeInTheDocument()
  })

  it('substitutes $from and $to time-range variables', () => {
    renderPanel({
      template: 'Range: $from to $to',
      timeRange: { begin: '2026-04-01T00:00:00Z', end: '2026-04-02T00:00:00Z' },
    })
    expect(
      screen.getByText('Range: 2026-04-01T00:00:00Z to 2026-04-02T00:00:00Z'),
    ).toBeInTheDocument()
  })

  it('row columns win name collisions against notebook variables', () => {
    renderPanel({
      template: 'value=$shared',
      row: buildRow({ x: '0', y: '0', z: '0', shared: 'from-row' }),
      variables: { shared: 'from-vars' },
    })
    expect(screen.getByText('value=from-row')).toBeInTheDocument()
  })

  it('leaves unresolved column references literal', () => {
    renderPanel({
      template: 'pid=$process_id',
      row: buildRow({ x: '0', y: '0', z: '0' }),
    })
    expect(screen.getByText('pid=$process_id')).toBeInTheDocument()
  })

  it('renders the process-logs link from a template using $process_id', () => {
    renderPanel({
      template: '[View process logs](/process?process_id=$process_id)',
      row: buildRow({ x: '0', y: '0', z: '0', process_id: 'abc-123' }),
    })
    const link = screen.getByRole('link', { name: 'View process logs' })
    expect(link).toHaveAttribute('href', expect.stringContaining('process_id=abc-123'))
  })
})
