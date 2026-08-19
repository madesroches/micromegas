/**
 * Page-level tests for `QueryDenyListPage`: renders rules from `list_query_denials()`, opens the
 * deny dialog and issues the right `deny_queries` SQL on confirm, and shows an error banner when
 * a mutation fails. Modeled on `ProcessMetricsPage.test.tsx`'s `streamQuery` mocking style.
 */
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { useEffect, useState } from 'react'
import { tableFromArrays } from 'apache-arrow'
import QueryDenyListPage from '../QueryDenyListPage'

vi.mock('@/lib/auth', () => ({
  useAuth: () => ({
    status: 'authenticated',
    user: { sub: 'admin', is_admin: true },
    error: null,
  }),
}))

vi.mock('@/hooks/usePageTitle', () => ({ usePageTitle: () => undefined }))

vi.mock('@/components/layout', () => ({
  PageLayout: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))

// useDefaultDataSource starts at '' and resolves asynchronously -- mirror that contract (see
// ProcessMetricsPage.test.tsx's identical comment) rather than returning a resolved value on the
// very first render.
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
  DataSourceField: () => null,
}))

const { mockStreamQuery } = vi.hoisted(() => ({ mockStreamQuery: vi.fn() }))

vi.mock('@/lib/arrow-stream', () => ({
  streamQuery: (...args: unknown[]) => mockStreamQuery(...args),
}))

function createMockGenerator<T>(results: T[]): AsyncGenerator<T> {
  let index = 0
  return {
    async next() {
      if (index < results.length) {
        return { done: false, value: results[index++] }
      }
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

function renderPage() {
  return render(
    <MemoryRouter>
      <QueryDenyListPage />
    </MemoryRouter>
  )
}

const oneRuleTable = tableFromArrays({
  rule_id: ['11111111-1111-1111-1111-111111111111'],
  created_at: [BigInt(1_700_000_000) * BigInt(1_000_000_000)],
  created_by: ['admin@example.com'],
  reason: ['alert re-firing'],
  match_expr: ["entrypoint = 'grafana-alert'"],
  last_hit_at: [BigInt(1_700_000_100) * BigInt(1_000_000_000)],
})

const emptyRulesTable = tableFromArrays({
  rule_id: [] as string[],
  created_at: [] as bigint[],
  created_by: [] as string[],
  reason: [] as string[],
  match_expr: [] as string[],
  last_hit_at: [] as (bigint | null)[],
})

beforeEach(() => {
  vi.clearAllMocks()
})

describe('QueryDenyListPage', () => {
  it('renders the empty state when there are no rules', async () => {
    mockStreamQuery.mockImplementation(() =>
      createMockGenerator([
        { type: 'schema', schema: emptyRulesTable.schema },
        { type: 'done' },
      ])
    )

    renderPage()

    await waitFor(() =>
      expect(screen.getByText(/No queries are currently denied/i)).toBeInTheDocument()
    )
  })

  it('lists rules from list_query_denials()', async () => {
    mockStreamQuery.mockImplementation(() =>
      createMockGenerator([
        { type: 'schema', schema: oneRuleTable.schema },
        ...oneRuleTable.batches.map((batch) => ({ type: 'batch' as const, batch })),
        { type: 'done' },
      ])
    )

    renderPage()

    await waitFor(() => expect(screen.getByText('alert re-firing')).toBeInTheDocument())
    expect(screen.getByText("entrypoint = 'grafana-alert'")).toBeInTheDocument()
    expect(screen.getByText('admin@example.com')).toBeInTheDocument()
  })

  it('opens the deny dialog and issues deny_queries on confirm', async () => {
    mockStreamQuery.mockImplementation(({ sql }: { sql: string }) => {
      if (sql.includes('deny_queries')) {
        return createMockGenerator([
          {
            type: 'schema',
            schema: tableFromArrays({ rule_id: ['22222222-2222-2222-2222-222222222222'] }).schema,
          },
          {
            type: 'batch',
            batch: tableFromArrays({ rule_id: ['22222222-2222-2222-2222-222222222222'] })
              .batches[0],
          },
          { type: 'done' },
        ])
      }
      return createMockGenerator([
        { type: 'schema', schema: emptyRulesTable.schema },
        { type: 'done' },
      ])
    })

    renderPage()

    await waitFor(() =>
      expect(screen.getByText(/No queries are currently denied/i)).toBeInTheDocument()
    )

    fireEvent.click(screen.getByRole('button', { name: /Deny a Query/i }))
    const textarea = await screen.findByPlaceholderText(/sql_hash =/i)
    fireEvent.change(textarea, { target: { value: "client = 'grafana'" } })
    const reasonInput = screen.getByPlaceholderText(/alert rule re-firing/i)
    fireEvent.change(reasonInput, { target: { value: 'test reason' } })

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Deny query' }))
    })

    await waitFor(() => {
      const denyCall = mockStreamQuery.mock.calls.find(
        (c) => (c[0] as { sql: string }).sql.includes('deny_queries')
      )
      expect(denyCall).toBeDefined()
      const sql = (denyCall![0] as { sql: string }).sql
      // The textarea's inner quotes are doubled on the way into the SQL literal.
      expect(sql).toContain("client = ''grafana''")
      expect(sql).toContain('test reason')
    })

    // Dialog closes and the list is reloaded once the rule is created.
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: 'Deny query' })).not.toBeInTheDocument()
    )
  })

  it('shows an error banner when the initial list query fails', async () => {
    mockStreamQuery.mockImplementation(() =>
      createMockGenerator([
        { type: 'error', error: { code: 'INTERNAL', message: 'boom', retryable: false } },
      ])
    )

    renderPage()

    await waitFor(() => expect(screen.getByText('boom')).toBeInTheDocument())
  })

  it('shows an inline error in the deny dialog when deny_queries fails', async () => {
    mockStreamQuery.mockImplementation(({ sql }: { sql: string }) => {
      if (sql.includes('deny_queries')) {
        return createMockGenerator([
          {
            type: 'error',
            error: { code: 'INVALID_SQL', message: 'query deny rule must reference at least one match-context column', retryable: false },
          },
        ])
      }
      return createMockGenerator([
        { type: 'schema', schema: emptyRulesTable.schema },
        { type: 'done' },
      ])
    })

    renderPage()

    await waitFor(() =>
      expect(screen.getByText(/No queries are currently denied/i)).toBeInTheDocument()
    )

    fireEvent.click(screen.getByRole('button', { name: /Deny a Query/i }))
    const textarea = await screen.findByPlaceholderText(/sql_hash =/i)
    fireEvent.change(textarea, { target: { value: 'true' } })
    const reasonInput = screen.getByPlaceholderText(/alert rule re-firing/i)
    fireEvent.change(reasonInput, { target: { value: 'test reason' } })

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Deny query' }))
    })

    await waitFor(() =>
      expect(screen.getByText(/at least one match-context column/i)).toBeInTheDocument()
    )
    // The dialog stays open on error so the admin can fix the expression.
    expect(screen.getByRole('button', { name: 'Deny query' })).toBeInTheDocument()
  })
})
