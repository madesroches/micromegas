/**
 * Test for ProcessMetricsPage's extraction guard (mirrors
 * PerformanceAnalysisPage.test.tsx's non-finite time/value case for issue
 * #1424). This is the third of three structurally identical guarded
 * extraction sites (useMetricsData.ts, PerformanceMetricsChart.tsx, and this
 * one) that drop rows with a non-finite time or value before handing data to
 * the chart.
 */
import { render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, useLocation, useSearchParams } from 'react-router'
import { useEffect, useState } from 'react'
import type { Mock } from 'vitest'
import { tableFromArrays } from 'apache-arrow'
import ProcessMetricsPage from '../ProcessMetricsPage'

// AuthGuard needs an authenticated user to render its children.
vi.mock('@/lib/auth', () => ({
  useAuth: () => ({
    status: 'authenticated',
    user: { sub: 'admin', is_admin: true },
    error: null,
  }),
}))

vi.mock('@/hooks/usePageTitle', () => ({ usePageTitle: () => undefined }))

// useDefaultDataSource always starts at '' and resolves asynchronously via
// its own effect (see useDataSourceState's render-time-diff logic, which
// relies on that contract to adopt the default once it resolves); mirror
// that here instead of returning a resolved value on the very first render.
vi.mock('@/hooks/useDefaultDataSource', () => ({
  useDefaultDataSource: () => {
    const [state, setState] = useState({ name: '', error: null })
    useEffect(() => {
      setState({ name: 'ds', error: null })
    }, [])
    return state
  },
}))

// PageLayout pulls in header/sidebar; stub it to a pass-through.
vi.mock('@/components/layout', () => ({
  PageLayout: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))

// Avoid DataSourceField's own network call (getDataSourceList) — irrelevant here.
vi.mock('@/components/DataSourceSelector', () => ({
  DataSourceField: () => null,
}))

// QueryEditor pulls in a full SQL editor; not exercised by this test.
vi.mock('@/components/QueryEditor', () => ({
  QueryEditor: () => <div data-testid="query-editor" />,
}))

// Captures the `data` prop MetricsChart is rendered with, so the test can
// assert on the chart points that survived extraction.
vi.mock('@/components/MetricsChart', () => ({
  MetricsChart: ({ data }: { data: { time: number; value: number }[] }) => (
    <div data-testid="metrics-chart" data-points={JSON.stringify(data)} />
  ),
}))

// Mock streamQuery function (same pattern as useMetricsData.test.ts)
const { mockStreamQuery } = vi.hoisted(() => ({ mockStreamQuery: vi.fn() }))

vi.mock('@/lib/arrow-stream', () => ({
  streamQuery: (...args: unknown[]) => mockStreamQuery(...args),
}))

// Helper to create a mock async generator, mirroring useMetricsData.test.ts
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

beforeEach(() => {
  vi.clearAllMocks()
  ;(useLocation as Mock).mockReturnValue({
    pathname: '/process_metrics',
    search: '?process_id=p1',
    hash: '',
    state: null,
    key: 'default',
  })
  ;(useSearchParams as Mock).mockReturnValue([
    new URLSearchParams('process_id=p1'),
    vi.fn(),
  ])

  const discoveryTable = tableFromArrays({
    name: ['DeltaTime'],
    target: ['cpu'],
    unit: ['ms'],
  })

  // Row 0: finite time and value -> kept
  // Row 1: finite time, non-finite value -> point dropped (time is valid)
  // Row 2: non-finite time -> row dropped entirely
  const metricsTable = tableFromArrays({
    time: new Float64Array([1000, 2000, Infinity]),
    value: new Float64Array([10, Infinity, 30]),
  })

  mockStreamQuery.mockImplementation(({ sql }: { sql: string }) => {
    const table = sql.includes('DISTINCT name') ? discoveryTable : metricsTable
    return createMockGenerator([
      { type: 'schema', schema: table.schema },
      ...table.batches.map((batch) => ({ type: 'batch' as const, batch })),
      { type: 'done' },
    ])
  })
})

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/process_metrics?process_id=p1']}>
      <ProcessMetricsPage />
    </MemoryRouter>
  )
}

describe('ProcessMetricsPage extraction guard', () => {
  it('drops rows with non-finite time/value (issue #1424)', async () => {
    renderPage()

    await waitFor(() => {
      const points = JSON.parse(
        screen.getByTestId('metrics-chart').getAttribute('data-points') || '[]'
      )
      expect(points).toEqual([{ time: 1000, value: 10 }])
    })
  })
})
