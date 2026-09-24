import { render, screen, fireEvent } from '@testing-library/react'
import { Int32, Table, Timestamp, TimeUnit, Utf8, vectorFromArray } from 'apache-arrow'
import { logMetadata } from '../LogCell'
import { formatLocalTime } from '../../log-utils'
import type { CellRendererProps, CellEditorProps } from '../../cell-registry'
import type { QueryCellConfig } from '../../notebook-types'

const LogCell = logMetadata.renderer

const createMockProps = (overrides: Partial<CellRendererProps> = {}): CellRendererProps => ({
  name: 'test-log',
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

interface LogRowInput {
  time: number
  level?: number
  target?: string
  msg: string
  request_id?: string
}

function buildLogTable(rows: LogRowInput[]): Table {
  return new Table({
    time: vectorFromArray(
      rows.map((r) => r.time),
      new Timestamp(TimeUnit.MILLISECOND, null),
    ),
    level: vectorFromArray(
      rows.map((r) => r.level ?? 4),
      new Int32(),
    ),
    target: vectorFromArray(
      rows.map((r) => r.target ?? 'app::mod'),
      new Utf8(),
    ),
    msg: vectorFromArray(
      rows.map((r) => r.msg),
      new Utf8(),
    ),
    request_id: vectorFromArray(
      rows.map((r) => r.request_id ?? ''),
      new Utf8(),
    ),
  })
}

describe('LogCell — collapse repeats', () => {
  it('collapses 5 identical rows + 1 distinct into 2 rendered lines, with a ×5 badge on the first', () => {
    const table = buildLogTable([
      { time: 1000, msg: 'retrying' },
      { time: 2000, msg: 'retrying' },
      { time: 3000, msg: 'retrying' },
      { time: 4000, msg: 'retrying' },
      { time: 5000, msg: 'retrying' },
      { time: 6000, msg: 'distinct' },
    ])
    render(<LogCell {...createMockProps({ data: [table] })} />)
    expect(screen.getAllByLabelText('Copy row')).toHaveLength(2)
    expect(screen.getByLabelText('Show 5 repeated rows')).toBeInTheDocument()
  })

  it('expands a run on badge click and collapses again on a second click', () => {
    const table = buildLogTable([
      { time: 1000, msg: 'retrying' },
      { time: 2000, msg: 'retrying' },
      { time: 3000, msg: 'retrying' },
      { time: 4000, msg: 'retrying' },
      { time: 5000, msg: 'retrying' },
      { time: 6000, msg: 'distinct' },
    ])
    render(<LogCell {...createMockProps({ data: [table] })} />)
    const badge = screen.getByLabelText('Show 5 repeated rows')
    expect(badge).toHaveAttribute('aria-expanded', 'false')

    fireEvent.click(badge)
    expect(screen.getByLabelText('Show 5 repeated rows')).toHaveAttribute('aria-expanded', 'true')
    expect(screen.getAllByLabelText('Copy row')).toHaveLength(6)

    fireEvent.click(screen.getByLabelText('Show 5 repeated rows'))
    expect(screen.getByLabelText('Show 5 repeated rows')).toHaveAttribute('aria-expanded', 'false')
    expect(screen.getAllByLabelText('Copy row')).toHaveLength(2)
  })

  it('shows every row with no badge when collapseRepeats is off, and the footer toggle flips it', () => {
    const table = buildLogTable([
      { time: 1000, msg: 'retrying' },
      { time: 2000, msg: 'retrying' },
      { time: 3000, msg: 'retrying' },
      { time: 4000, msg: 'retrying' },
      { time: 5000, msg: 'retrying' },
      { time: 6000, msg: 'distinct' },
    ])
    const onOptionsChange = vi.fn()
    render(
      <LogCell
        {...createMockProps({
          data: [table],
          options: { collapseRepeats: false },
          onOptionsChange,
        })}
      />,
    )
    expect(screen.getAllByLabelText('Copy row')).toHaveLength(6)
    expect(screen.queryByLabelText(/Show \d+ repeated rows/)).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: /Collapse repeats/ }))
    expect(onOptionsChange).toHaveBeenCalledWith(
      expect.objectContaining({ collapseRepeats: true }),
    )
  })

  it('groups rows that differ only in an ignored column', () => {
    const table = buildLogTable([
      { time: 1000, msg: 'retrying', request_id: 'a' },
      { time: 2000, msg: 'retrying', request_id: 'b' },
      { time: 3000, msg: 'retrying', request_id: 'c' },
    ])
    render(
      <LogCell
        {...createMockProps({
          data: [table],
          options: { collapseIgnoreColumns: ['request_id'] },
        })}
      />,
    )
    expect(screen.getAllByLabelText('Copy row')).toHaveLength(1)
    expect(screen.getByLabelText('Show 3 repeated rows')).toBeInTheDocument()
  })

  it('paginates over groups, not raw rows: 60 rows forming 2 groups show no pagination bar', () => {
    const rows: LogRowInput[] = []
    for (let i = 0; i < 30; i++) rows.push({ time: i, msg: 'first-run' })
    for (let i = 30; i < 60; i++) rows.push({ time: i, msg: 'second-run' })
    const table = buildLogTable(rows)
    render(<LogCell {...createMockProps({ data: [table], options: { pageSize: 50 } })} />)
    expect(screen.getAllByLabelText('Copy row')).toHaveLength(2)
    expect(screen.queryByTitle('First page')).not.toBeInTheDocument()
  })

  it('badge title orders the timestamps earlier -> later regardless of row order (ASC and DESC fixtures)', () => {
    const ascTable = buildLogTable([
      { time: 1000, msg: 'retrying' },
      { time: 2000, msg: 'retrying' },
      { time: 3000, msg: 'retrying' },
    ])
    const { unmount } = render(<LogCell {...createMockProps({ data: [ascTable] })} />)
    const ascTitle = screen.getByLabelText('Show 3 repeated rows').getAttribute('title')
    expect(ascTitle).toBe(
      `3 identical rows, ${formatLocalTime(1000)} → ${formatLocalTime(3000)}`,
    )
    unmount()

    const descTable = buildLogTable([
      { time: 3000, msg: 'retrying' },
      { time: 2000, msg: 'retrying' },
      { time: 1000, msg: 'retrying' },
    ])
    render(<LogCell {...createMockProps({ data: [descTable] })} />)
    const descTitle = screen.getByLabelText('Show 3 repeated rows').getAttribute('title')
    expect(descTitle).toBe(
      `3 identical rows, ${formatLocalTime(1000)} → ${formatLocalTime(3000)}`,
    )
  })
})

describe('LogCellEditor — ignore-columns chips', () => {
  const LogCellEditor = logMetadata.EditorComponent

  const createEditorProps = (overrides: Partial<CellEditorProps> = {}): CellEditorProps => ({
    config: {
      type: 'log' as const,
      name: 'my_log',
      layout: { height: 300 },
      sql: 'SELECT time, level, target, msg FROM log_entries',
      options: {},
    } as QueryCellConfig,
    onChange: vi.fn(),
    variables: {},
    timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
    cellResults: {},
    cellSelections: {},
    availableColumns: ['time', 'level', 'target', 'msg', 'request_id'],
    ...overrides,
  })

  it('calls onChange with the updated ignore list when a chip is toggled', () => {
    const onChange = vi.fn()
    render(<LogCellEditor {...createEditorProps({ onChange })} />)
    fireEvent.click(screen.getByRole('button', { name: 'request_id' }))
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        options: expect.objectContaining({ collapseIgnoreColumns: ['request_id'] }),
      }),
    )
  })

  it('disables the time chip', () => {
    render(<LogCellEditor {...createEditorProps()} />)
    expect(screen.getByRole('button', { name: 'time' })).toBeDisabled()
  })

  it('shows a hint instead of chips when availableColumns is empty', () => {
    render(<LogCellEditor {...createEditorProps({ availableColumns: undefined })} />)
    expect(screen.getByText('Run the query to choose columns')).toBeInTheDocument()
  })
})
