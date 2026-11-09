/**
 * Page-level tests for `ViewSetsPage`: admin gating, list and detail rendering, and the
 * create/save/drop flows. Mocks `streamQuery` like `QueryDenyListPage.test.tsx`.
 */
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { useEffect, useState } from 'react'
import { tableFromArrays } from 'apache-arrow'
import ViewSetsPage from '../ViewSetsPage'

const authState = vi.hoisted(() => ({
  user: { sub: 'admin', is_admin: true } as { sub: string; is_admin?: boolean },
}))

vi.mock('@/lib/auth', () => ({
  useAuth: () => ({
    status: 'authenticated',
    user: authState.user,
    error: null,
  }),
}))

vi.mock('@/hooks/usePageTitle', () => ({ usePageTitle: () => undefined }))

vi.mock('@/components/layout', () => ({
  PageLayout: ({ children, onRefresh }: { children: React.ReactNode; onRefresh?: () => void }) => (
    <div>
      <button onClick={onRefresh}>refresh page</button>
      {children}
    </div>
  ),
}))

vi.mock('@/hooks/useDefaultDataSource', () => ({
  useDefaultDataSource: () => {
    const [state, setState] = useState({ name: '', error: null })
    useEffect(() => {
      setState({ name: 'ds', error: null })
    }, [])
    return state
  },
}))

vi.mock('@/components/DataSourceSelector', () => ({
  DataSourceField: ({ onChange }: { onChange: (v: string) => void }) => (
    <button onClick={() => onChange('other-ds')}>switch data source</button>
  ),
}))

const { mockStreamQuery } = vi.hoisted(() => ({ mockStreamQuery: vi.fn() }))

vi.mock('@/lib/arrow-stream', () => ({
  streamQuery: (...args: unknown[]) => mockStreamQuery(...args),
}))

function createMockGenerator<T>(results: T[]): AsyncGenerator<T> {
  let index = 0
  return {
    async next() {
      if (index < results.length) return { done: false, value: results[index++] }
      return { done: true, value: undefined }
    },
    async return(value?: unknown) {
      return { done: true, value: value as T }
    },
    async throw(e?: unknown) {
      throw e
    },
    [Symbol.asyncIterator]() {
      return this
    },
  } as AsyncGenerator<T>
}

function tableStream(table: ReturnType<typeof tableFromArrays>) {
  return createMockGenerator([
    { type: 'schema' as const, schema: table.schema },
    ...table.batches.map((batch) => ({ type: 'batch' as const, batch })),
    { type: 'done' as const },
  ])
}

const DEFINITION = 'CREATE MATERIALIZED VIEW log_stats WITH (update_group = 1)'

const definitionsTable = tableFromArrays({
  view_set_name: ['log_stats', 'log_entries'],
  definition_sql: [DEFINITION, null],
  update_group: [1, null],
  updated_at: [BigInt(1_700_000_000) * BigInt(1_000_000_000), null],
  updated_by: ['admin', null],
  source: ['ddl', 'builtin'],
})

const loadedTable = tableFromArrays({
  view_set_name: ['log_stats', 'log_entries'],
  description: ['Usage guide', null],
  columns: ['time_bin: Int64\ncount: Int64', 'time: Int64'],
  sort_order: ['time_bin', null],
})

const streamsTable = tableFromArrays({ streams: [true], sort_exec_line: [''] })
const statusTable = tableFromArrays({
  view_set_name: ['log_stats'],
  status: ['replaced'],
})

function sqlOf(call: unknown[]): string {
  return (call[0] as { sql: string }).sql
}

function defaultImpl({ sql }: { sql: string }) {
  if (sql.includes('list_view_set_definitions')) return tableStream(definitionsTable)
  if (sql.includes('list_view_sets')) return tableStream(loadedTable)
  if (sql.includes('check_view_merge')) return tableStream(streamsTable)
  return tableStream(statusTable)
}

function renderPage() {
  return render(
    <MemoryRouter>
      <ViewSetsPage />
    </MemoryRouter>,
  )
}

async function selectLogStats() {
  fireEvent.click(await screen.findByText('log_stats'))
  await screen.findByText(/Updated/)
}

beforeEach(() => {
  vi.clearAllMocks()
  authState.user = { sub: 'admin', is_admin: true }
  mockStreamQuery.mockImplementation(defaultImpl)
})

describe('ViewSetsPage', () => {
  it('shows Access Denied and issues no query for a non-admin', async () => {
    authState.user = { sub: 'user', is_admin: false }
    renderPage()
    await screen.findByText(/Access Denied/i)
    expect(mockStreamQuery).not.toHaveBeenCalled()
  })

  it('reloads partition stats on Refresh while the Partitions tab is open', async () => {
    renderPage()
    await selectLogStats()
    fireEvent.click(screen.getByRole('button', { name: 'partitions' }))
    const statsCalls = () =>
      mockStreamQuery.mock.calls.filter((c) => sqlOf(c).includes('FROM list_partitions()')).length
    await waitFor(() => expect(statsCalls()).toBe(1))
    fireEvent.click(screen.getByText('refresh page'))
    await waitFor(() => expect(statsCalls()).toBe(2))
  })

  it('lists view sets with their health, all through the admin endpoint', async () => {
    renderPage()
    await screen.findByText('log_stats')
    expect(screen.getByText('log_entries')).toBeInTheDocument()
    expect(screen.getByText('ok')).toBeInTheDocument()
    expect(screen.getByText('read-only')).toBeInTheDocument()
    for (const call of mockStreamQuery.mock.calls) {
      expect((call[0] as { adminMode?: boolean }).adminMode).toBe(true)
    }
  })

  it('shows the schema tab columns and description', async () => {
    renderPage()
    await selectLogStats()
    fireEvent.click(screen.getByRole('button', { name: 'schema' }))
    expect(await screen.findByText('count')).toBeInTheDocument()
    expect(screen.getByText('Usage guide')).toBeInTheDocument()
  })

  it('gates Save behind a confirm dialog that warns about content-only changes', async () => {
    renderPage()
    await selectLogStats()
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await screen.findByText(/does not rebuild anything/i)
    expect(mockStreamQuery.mock.calls.some((c) => sqlOf(c).startsWith('CREATE'))).toBe(false)

    await act(async () => {
      fireEvent.click(screen.getAllByRole('button', { name: 'Save' })[0])
    })
    await waitFor(() =>
      expect(
        mockStreamQuery.mock.calls.some((c) =>
          sqlOf(c).startsWith('CREATE OR REPLACE MATERIALIZED VIEW log_stats'),
        ),
      ).toBe(true),
    )
  })

  it('closes the editor and drops its text when the data source changes', async () => {
    renderPage()
    await selectLogStats()
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    fireEvent.change(screen.getByRole('textbox'), {
      target: { value: 'stale text from ds' },
    })

    fireEvent.click(screen.getByRole('button', { name: 'switch data source' }))

    await screen.findByText('log_stats')
    expect(screen.queryByRole('textbox')).toBeNull()
    expect(screen.queryByText('stale text from ds')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Save' })).toBeNull()
  })

  it('shows unknown health when the loaded view sets query fails', async () => {
    mockStreamQuery.mockImplementation(({ sql }: { sql: string }) => {
      if (sql.includes('list_view_set_definitions')) return tableStream(definitionsTable)
      if (sql.includes('list_view_sets')) throw new Error('boom')
      return tableStream(statusTable)
    })
    renderPage()
    await screen.findByText('log_stats')
    expect(screen.getByText('unknown')).toBeInTheDocument()
    expect(screen.queryByText('failed to load')).toBeNull()
  })

  it('refuses to save a statement that names a different view set', async () => {
    renderPage()
    await selectLogStats()
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    fireEvent.change(screen.getByRole('textbox'), {
      target: { value: DEFINITION.replace('log_stats', 'other') },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await screen.findByText(/must define "log_stats"/)
    expect(screen.queryByText(/does not rebuild anything/i)).toBeNull()
    expect(mockStreamQuery.mock.calls.some((c) => sqlOf(c).startsWith('CREATE'))).toBe(false)
  })

  it('refetches the lists after a successful save', async () => {
    renderPage()
    await selectLogStats()
    const before = mockStreamQuery.mock.calls.filter((c) =>
      sqlOf(c).includes('list_view_set_definitions'),
    ).length
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await act(async () => {
      fireEvent.click(screen.getAllByRole('button', { name: 'Save' })[0])
    })
    await waitFor(() =>
      expect(
        mockStreamQuery.mock.calls.filter((c) => sqlOf(c).includes('list_view_set_definitions'))
          .length,
      ).toBeGreaterThan(before),
    )
  })

  it('keeps the editor text and shows the server error when a save fails', async () => {
    mockStreamQuery.mockImplementation((params: { sql: string }) => {
      if (params.sql.startsWith('CREATE OR REPLACE')) {
        return createMockGenerator([
          {
            type: 'error',
            error: {
              code: 'INVALID_SQL',
              message: 'parse error near WITH',
              retryable: false,
            },
          },
        ])
      }
      return defaultImpl(params)
    })
    renderPage()
    await selectLogStats()
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    const editor = screen.getByRole('textbox') as HTMLTextAreaElement
    fireEvent.change(editor, { target: { value: `${DEFINITION} -- edited` } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await act(async () => {
      fireEvent.click(screen.getAllByRole('button', { name: 'Save' })[0])
    })

    await screen.findByText(/parse error near WITH/)
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe(
      `${DEFINITION} -- edited`,
    )
  })

  it('gates Drop behind a confirm dialog and shows the server refusal', async () => {
    mockStreamQuery.mockImplementation((params: { sql: string }) => {
      if (params.sql.startsWith('DROP')) {
        return createMockGenerator([
          {
            type: 'error',
            error: {
              code: 'INVALID_SQL',
              message: 'read by other_view',
              retryable: false,
            },
          },
        ])
      }
      return defaultImpl(params)
    })
    renderPage()
    await selectLogStats()
    fireEvent.click(screen.getByRole('button', { name: 'Drop' }))
    await screen.findByText(/Its partitions are retired/)
    expect(mockStreamQuery.mock.calls.some((c) => sqlOf(c).startsWith('DROP'))).toBe(false)

    await act(async () => {
      fireEvent.click(screen.getAllByRole('button', { name: 'Drop' })[0])
    })
    await screen.findByText(/read by other_view/)
    expect(
      mockStreamQuery.mock.calls.some((c) => sqlOf(c) === 'DROP MATERIALIZED VIEW log_stats'),
    ).toBe(true)
  })

  it('creates a view set with a plain CREATE', async () => {
    renderPage()
    await screen.findByText('log_stats')
    fireEvent.click(screen.getByRole('button', { name: /Create View Set/i }))
    await screen.findByRole('heading', { name: 'Create View Set' })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Create' }))
    })
    await waitFor(() =>
      expect(
        mockStreamQuery.mock.calls.some((c) => sqlOf(c).startsWith('CREATE MATERIALIZED VIEW')),
      ).toBe(true),
    )
    await waitFor(() =>
      expect(screen.queryByRole('heading', { name: 'Create View Set' })).not.toBeInTheDocument(),
    )
  })

  it('sends check_view_merge from Check and warns when the merge does not stream', async () => {
    mockStreamQuery.mockImplementation((params: { sql: string }) =>
      params.sql.includes('check_view_merge')
        ? tableStream(
            tableFromArrays({
              streams: [false],
              sort_exec_line: ['SortExec: x'],
            }),
          )
        : defaultImpl(params),
    )
    renderPage()
    await screen.findByText('log_stats')
    fireEvent.click(screen.getByRole('button', { name: /Create View Set/i }))
    await screen.findByRole('heading', { name: 'Create View Set' })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Check' }))
    })
    await screen.findByText('The merge does not stream')
    expect(mockStreamQuery.mock.calls.some((c) => sqlOf(c).includes('check_view_merge'))).toBe(true)
  })

  it('hides a Check result once the SQL is edited', async () => {
    renderPage()
    await screen.findByText('log_stats')
    fireEvent.click(screen.getByRole('button', { name: /Create View Set/i }))
    await screen.findByRole('heading', { name: 'Create View Set' })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Check' }))
    })
    await screen.findByText('The merge streams its input.')
    fireEvent.change(screen.getByRole('textbox'), {
      target: { value: 'CREATE MATERIALIZED VIEW z' },
    })
    expect(screen.queryByText('The merge streams its input.')).toBeNull()
  })

  it('warns on the Definition tab when the merge does not stream', async () => {
    mockStreamQuery.mockImplementation((params: { sql: string }) =>
      params.sql.includes('check_view_merge')
        ? tableStream(
            tableFromArrays({
              streams: [false],
              sort_exec_line: ['SortExec: x'],
            }),
          )
        : defaultImpl(params),
    )
    renderPage()
    await selectLogStats()
    await screen.findByText('The merge does not stream')
  })

  it('closes the drop dialog, clears the selection and refetches after a drop', async () => {
    renderPage()
    await selectLogStats()
    const defCalls = () =>
      mockStreamQuery.mock.calls.filter((c) => sqlOf(c).includes('list_view_set_definitions'))
        .length
    const before = defCalls()
    fireEvent.click(screen.getByRole('button', { name: 'Drop' }))
    await screen.findByText(/Its partitions are retired/)
    await act(async () => {
      fireEvent.click(screen.getAllByRole('button', { name: 'Drop' })[0])
    })
    await waitFor(() => expect(screen.queryByText(/Its partitions are retired/)).toBeNull())
    expect(screen.queryByText(/Updated/)).toBeNull()
    await waitFor(() => expect(defCalls()).toBeGreaterThan(before))
  })

  it('keeps the create dialog open with the server error when create fails', async () => {
    mockStreamQuery.mockImplementation((params: { sql: string }) =>
      params.sql.startsWith('CREATE MATERIALIZED VIEW')
        ? createMockGenerator([
            {
              type: 'error',
              error: {
                code: 'INVALID_SQL',
                message: 'bad ddl',
                retryable: false,
              },
            },
          ])
        : defaultImpl(params),
    )
    renderPage()
    await screen.findByText('log_stats')
    fireEvent.click(screen.getByRole('button', { name: /Create View Set/i }))
    await screen.findByRole('heading', { name: 'Create View Set' })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Create' }))
    })
    await screen.findByText(/bad ddl/)
    expect(screen.getByRole('heading', { name: 'Create View Set' })).toBeInTheDocument()
  })
})
