// computeTabState is a pure function with no rendering dependencies, but importing
// ScreenPage.tsx still pulls in every registered renderer transitively (including
// MetricsRenderer -> uPlot, which touches matchMedia at import time). Stub the
// renderer registry module out so this test stays a lightweight unit test of the
// pure function, not an integration test of the whole renderer graph.
vi.mock('@/lib/screen-renderers/init', () => ({ SCREEN_RENDERERS: {} }))

import { computeTabState } from '../ScreenPage'

describe('computeTabState', () => {
  it('is busy whenever isExecuting is true, regardless of hasError', () => {
    expect(computeTabState(true, false)).toBe('busy')
    expect(computeTabState(true, true)).toBe('busy')
  })

  it('is error when idle with an error', () => {
    expect(computeTabState(false, true)).toBe('error')
  })

  it('is idle when idle with no error', () => {
    expect(computeTabState(false, false)).toBe('idle')
  })
})
