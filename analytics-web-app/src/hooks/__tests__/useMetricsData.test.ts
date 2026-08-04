/**
 * Tests for useMetricsData hook — non-finite value/time handling
 */
import { renderHook, act } from '@testing-library/react'
import { tableFromArrays } from 'apache-arrow'

// Mock streamQuery function (same pattern as useStreamQuery.test.ts)
const { mockStreamQuery } = vi.hoisted(() => ({ mockStreamQuery: vi.fn() }))

vi.mock('@/lib/arrow-stream', () => ({
  streamQuery: (...args: unknown[]) => mockStreamQuery(...args),
}))

import { useMetricsData } from '../useMetricsData'

// Helper to create a mock async generator, mirroring useStreamQuery.test.ts
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

describe('useMetricsData', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('should drop rows with non-finite value or time, keeping only finite rows', async () => {
    // Row 0: finite time and value -> kept
    // Row 1: finite time, non-finite value -> point dropped (time is valid)
    // Row 2: non-finite time -> row dropped entirely
    const table = tableFromArrays({
      time: new Float64Array([1000, 2000, Infinity]),
      value: new Float64Array([10, Infinity, 30]),
    })

    mockStreamQuery.mockReturnValue(
      createMockGenerator([
        { type: 'schema', schema: table.schema },
        ...table.batches.map(batch => ({ type: 'batch' as const, batch })),
        { type: 'done' },
      ])
    )

    const { result } = renderHook(() =>
      useMetricsData({
        processId: 'p1',
        measureName: 'm1',
        binInterval: '1 minute',
        apiTimeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
      })
    )

    await act(async () => {
      result.current.execute()
      await new Promise(resolve => setTimeout(resolve, 0))
    })

    expect(result.current.chartData).toEqual([{ time: 1000, value: 10 }])
  })
})
