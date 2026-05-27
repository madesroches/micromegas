/**
 * Characterization tests for `PerformanceAnalysisPage`'s fetch-orchestration
 * gate. These pin the subtle effect ordering that the #1089 refactor must
 * preserve byte-for-byte:
 *   - on mount: discovery + thread-coverage + event-count fire; the measure
 *     auto-selects; the metrics query executes after discovery.
 *   - on time-range change (while metrics are complete): discovery and
 *     thread-coverage re-fetch exactly once, no fetch-before-discovery.
 *   - on refresh: discovery + thread-coverage re-fetch.
 *
 * The chart/timeline/editor and the data layer are stubbed; what is under test
 * is the page's orchestration, not pixel output.
 */
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react'
import { MemoryRouter, useLocation, useSearchParams } from 'react-router-dom'
import PerformanceAnalysisPage from '../PerformanceAnalysisPage'

// AuthGuard needs an authenticated user to render its children.
jest.mock('@/lib/auth', () => ({
  useAuth: () => ({
    status: 'authenticated',
    user: { sub: 'admin', is_admin: true },
    error: null,
  }),
}))

jest.mock('@/lib/config', () => ({
  getConfig: () => ({ basePath: '' }),
  appLink: (path: string) => path,
}))

jest.mock('@/hooks/usePageTitle', () => ({ usePageTitle: () => undefined }))

jest.mock('@/hooks/useDefaultDataSource', () => ({
  useDefaultDataSource: () => ({ name: 'ds', error: null }),
}))

// PageLayout pulls in header/sidebar; stub it to a pass-through that also
// surfaces the controls the page wires up so the test can trigger a
// time-range change and a refresh.
jest.mock('@/components/layout', () => ({
  PageLayout: ({
    children,
    rightPanel,
    onRefresh,
    timeRangeControl,
  }: {
    children: React.ReactNode
    rightPanel?: React.ReactNode
    onRefresh?: () => void
    timeRangeControl?: { onTimeRangeChange: (from: string, to: string) => void }
  }) => (
    <div>
      <button data-testid="trigger-refresh" onClick={() => onRefresh?.()}>
        refresh
      </button>
      <button
        data-testid="trigger-time-change"
        onClick={() => timeRangeControl?.onTimeRangeChange('now-2h', 'now')}
      >
        change time
      </button>
      <div data-testid="right-panel">{rightPanel}</div>
      {children}
    </div>
  ),
}))

jest.mock('@/components/MetricsChart', () => ({
  MetricsChart: () => <div data-testid="metrics-chart" />,
}))

jest.mock('@/components/ThreadCoverageTimeline', () => ({
  ThreadCoverageTimeline: () => <div data-testid="thread-coverage" />,
}))

jest.mock('@/components/QueryEditor', () => ({
  QueryEditor: () => <div data-testid="query-editor" />,
}))

jest.mock('@/lib/perfetto-trace', () => ({
  fetchPerfettoTrace: jest.fn(),
  triggerTraceDownload: jest.fn(),
}))

jest.mock('@/lib/perfetto', () => ({
  openInPerfetto: jest.fn(),
  PerfettoError: class PerfettoError extends Error {},
}))

// A controllable metrics hook: always "complete" so the time-range gate is
// armed (hasLoaded === true). The returned object is closure-stable to mirror
// the real hook (whose outputs come from useState/useMemo) — otherwise the
// page's view-state lift effect would re-fire every render and loop.
const mockExecute = jest.fn()
jest.mock('@/hooks/useMetricsData', () => {
  // Built lazily on first call (so mockExecute is initialized) and cached so
  // the reference is stable across renders.
  let metrics: Record<string, unknown> | null = null
  return {
    useMetricsData: () => {
      if (!metrics) {
        metrics = {
          chartData: [{ time: 1000, value: 5 }],
          availablePropertyKeys: [] as string[],
          getPropertyTimeline: () => ({ segments: [] }),
          propertyParseErrors: [] as string[],
          isLoading: false,
          isComplete: true,
          error: null,
          execute: mockExecute,
        }
      }
      return metrics
    },
  }
})

// Fake RecordBatch with the minimal surface the page touches.
function fakeBatch(rows: Record<string, unknown>[]) {
  return {
    numRows: rows.length,
    get: (i: number) => rows[i],
    schema: { fields: [] },
  }
}

const executeStreamQuery = jest.fn()
jest.mock('@/lib/arrow-stream', () => ({
  executeStreamQuery: (...args: unknown[]) => executeStreamQuery(...args),
}))

function classifySql(sql: string): 'discovery' | 'coverage' | 'count' | 'other' {
  if (sql.includes('DISTINCT name')) return 'discovery'
  if (sql.includes('event_count')) return 'count'
  if (sql.includes('begin_time') && sql.includes("array_has")) return 'coverage'
  return 'other'
}

function callsByKind() {
  const kinds = executeStreamQuery.mock.calls.map((c) => classifySql((c[0] as { sql: string }).sql))
  return {
    discovery: kinds.filter((k) => k === 'discovery').length,
    coverage: kinds.filter((k) => k === 'coverage').length,
    count: kinds.filter((k) => k === 'count').length,
  }
}

beforeEach(() => {
  jest.clearAllMocks()
  ;(useLocation as jest.Mock).mockReturnValue({
    pathname: '/performance',
    search: '?process_id=p1',
    hash: '',
    state: null,
    key: 'default',
  })
  ;(useSearchParams as jest.Mock).mockReturnValue([
    new URLSearchParams('process_id=p1'),
    jest.fn(),
  ])

  executeStreamQuery.mockImplementation(async ({ sql }: { sql: string }) => {
    switch (classifySql(sql)) {
      case 'discovery':
        return {
          schema: null,
          batches: [
            fakeBatch([
              { name: 'DeltaTime', target: 'cpu', unit: 'ms' },
              { name: 'FrameTime', target: 'cpu', unit: 'ms' },
            ]),
          ],
          error: null,
        }
      case 'coverage':
        return {
          schema: null,
          batches: [
            fakeBatch([
              { stream_id: 's1', thread_name: 'main', begin_time: 0, end_time: 100 },
            ]),
          ],
          error: null,
        }
      case 'count':
        return {
          schema: null,
          batches: [fakeBatch([{ event_count: 42 }])],
          error: null,
        }
      default:
        return { schema: null, batches: [], error: null }
    }
  })
})

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/performance?process_id=p1']}>
      <PerformanceAnalysisPage />
    </MemoryRouter>
  )
}

describe('PerformanceAnalysisPage orchestration', () => {
  it('on mount: discovers, auto-selects, executes metrics, loads thread coverage', async () => {
    renderPage()

    await waitFor(() => {
      const c = callsByKind()
      expect(c.discovery).toBe(1)
      expect(c.coverage).toBe(1)
      expect(c.count).toBe(1)
    })

    // Auto-select prefers DeltaTime.
    const select = (await screen.findByRole('combobox')) as HTMLSelectElement
    expect(select.value).toBe('DeltaTime')

    // Metrics query executed after discovery completed.
    await waitFor(() => expect(mockExecute).toHaveBeenCalled())

    // Thread coverage timeline renders (chartData present + threads present).
    expect(await screen.findByTestId('thread-coverage')).toBeInTheDocument()
  })

  it('time-range change re-fetches discovery + coverage exactly once (gated on metrics complete)', async () => {
    renderPage()
    await waitFor(() => expect(callsByKind().discovery).toBe(1))

    executeStreamQuery.mockClear()
    await act(async () => {
      fireEvent.click(screen.getByTestId('trigger-time-change'))
    })

    await waitFor(() => {
      const c = callsByKind()
      expect(c.discovery).toBe(1)
      expect(c.coverage).toBe(1)
    })
    // No extra discovery fetches sneak in.
    await new Promise((r) => setTimeout(r, 50))
    expect(callsByKind().discovery).toBe(1)
  })

  it('refresh re-fetches discovery + thread coverage', async () => {
    renderPage()
    await waitFor(() => expect(callsByKind().discovery).toBe(1))

    executeStreamQuery.mockClear()
    await act(async () => {
      fireEvent.click(screen.getByTestId('trigger-refresh'))
    })

    await waitFor(() => {
      const c = callsByKind()
      expect(c.discovery).toBe(1)
      expect(c.coverage).toBe(1)
    })
  })
})
