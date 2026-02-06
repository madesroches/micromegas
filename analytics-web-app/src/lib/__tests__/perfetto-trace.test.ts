import { fetchPerfettoTrace } from '../perfetto-trace'
import type { StreamResult } from '../arrow-stream'
import type { RecordBatch } from 'apache-arrow'

jest.mock('../arrow-stream', () => ({
  streamQuery: jest.fn(),
}))

import { streamQuery } from '../arrow-stream'
const mockStreamQuery = streamQuery as jest.MockedFunction<typeof streamQuery>

function makeBatch(data: Uint8Array, chunkId: number = 0): RecordBatch {
  return {
    numRows: 1,
    getChild(name: string) {
      if (name === 'chunk_id') {
        return { get: (i: number) => i === 0 ? chunkId : null }
      }
      if (name === 'chunk_data') {
        return { get: (i: number) => i === 0 ? data : null }
      }
      return null
    },
  } as unknown as RecordBatch
}

async function* fakeStream(results: StreamResult[]): AsyncGenerator<StreamResult> {
  for (const r of results) {
    yield r
  }
}

describe('fetchPerfettoTrace', () => {
  beforeEach(() => {
    jest.clearAllMocks()
  })

  it('should concatenate chunks in order', async () => {
    const chunk1 = new Uint8Array([1, 2, 3])
    const chunk2 = new Uint8Array([4, 5, 6])

    mockStreamQuery.mockReturnValue(
      fakeStream([
        { type: 'batch', batch: makeBatch(chunk1, 0) },
        { type: 'batch', batch: makeBatch(chunk2, 1) },
        { type: 'done' },
      ])
    )

    const buffer = await fetchPerfettoTrace({
      processId: 'proc-1',
      spanType: 'both',
      timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
    })

    expect(new Uint8Array(buffer)).toEqual(new Uint8Array([1, 2, 3, 4, 5, 6]))
  })

  it('should call onProgress with byte counts', async () => {
    const chunk = new Uint8Array([10, 20, 30])
    const onProgress = jest.fn()

    mockStreamQuery.mockReturnValue(
      fakeStream([
        { type: 'batch', batch: makeBatch(chunk) },
        { type: 'done' },
      ])
    )

    await fetchPerfettoTrace({
      processId: 'proc-1',
      spanType: 'both',
      timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
      onProgress,
    })

    expect(onProgress).toHaveBeenCalled()
    expect(onProgress.mock.calls[0][0]).toMatch(/Downloading trace.*MB received/)
  })

  it('should throw on stream error', async () => {
    mockStreamQuery.mockReturnValue(
      fakeStream([
        {
          type: 'error',
          error: { code: 'INTERNAL' as const, message: 'Something broke', retryable: false },
        },
      ])
    )

    await expect(
      fetchPerfettoTrace({
        processId: 'proc-1',
        spanType: 'both',
        timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
      })
    ).rejects.toThrow('Something broke')
  })

  it('should throw on empty stream', async () => {
    mockStreamQuery.mockReturnValue(fakeStream([{ type: 'done' }]))

    await expect(
      fetchPerfettoTrace({
        processId: 'proc-1',
        spanType: 'both',
        timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
      })
    ).rejects.toThrow('No trace data generated')
  })

  it('should throw on out-of-order chunks', async () => {
    mockStreamQuery.mockReturnValue(
      fakeStream([
        { type: 'batch', batch: makeBatch(new Uint8Array([1]), 0) },
        { type: 'batch', batch: makeBatch(new Uint8Array([2]), 5) },
        { type: 'done' },
      ])
    )

    await expect(
      fetchPerfettoTrace({
        processId: 'proc-1',
        spanType: 'both',
        timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
      })
    ).rejects.toThrow('Chunk 5 received, expected 1')
  })

  it('should abort between batches when signal is triggered', async () => {
    const controller = new AbortController()

    async function* abortingStream(): AsyncGenerator<StreamResult> {
      yield { type: 'batch', batch: makeBatch(new Uint8Array([1]), 0) }
      controller.abort('user cancelled')
      yield { type: 'batch', batch: makeBatch(new Uint8Array([2]), 1) }
      yield { type: 'done' }
    }

    mockStreamQuery.mockReturnValue(abortingStream())

    await expect(
      fetchPerfettoTrace({
        processId: 'proc-1',
        spanType: 'both',
        timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
        signal: controller.signal,
      })
    ).rejects.toThrow('user cancelled')
  })

  it('should forward abort signal to streamQuery', async () => {
    const controller = new AbortController()

    mockStreamQuery.mockReturnValue(
      fakeStream([
        { type: 'batch', batch: makeBatch(new Uint8Array([1])) },
        { type: 'done' },
      ])
    )

    await fetchPerfettoTrace({
      processId: 'proc-1',
      spanType: 'thread',
      timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
      signal: controller.signal,
    })

    expect(mockStreamQuery).toHaveBeenCalledWith(
      expect.objectContaining({ sql: expect.stringContaining('thread') }),
      controller.signal
    )
  })

  it('should build correct SQL with parameters', async () => {
    mockStreamQuery.mockReturnValue(
      fakeStream([
        { type: 'batch', batch: makeBatch(new Uint8Array([1])) },
        { type: 'done' },
      ])
    )

    await fetchPerfettoTrace({
      processId: 'my-process',
      spanType: 'async',
      timeRange: { begin: '2024-06-01T00:00:00Z', end: '2024-06-02T00:00:00Z' },
    })

    const call = mockStreamQuery.mock.calls[0]
    const sql = call[0].sql
    expect(sql).toContain("'my-process'")
    expect(sql).toContain("'async'")
    expect(sql).toContain("'2024-06-01T00:00:00Z'")
    expect(sql).toContain("'2024-06-02T00:00:00Z'")
  })
})
