/**
 * Integration tests for Arrow IPC streaming with Utf8View / BinaryView columns.
 *
 * These tests use the real apache-arrow library (not mocked) to verify that
 * the streaming path (`streamQuery` -> `RecordBatchReader.from(ipcByteStream())`)
 * decodes view-type columns correctly, both uncompressed and LZ4_FRAME-compressed
 * (the framing/compression `stream_query.rs` actually produces on the server).
 *
 * This is the direct regression test for the reported
 * `Unrecognized type: "undefined" (24)` error over the exact framing and
 * chunk boundaries the server uses.
 */

import { authenticatedFetch } from '@/lib/api'
import { streamQuery, StreamResult } from '../arrow-stream'
import { DataType } from 'apache-arrow'
import type { MockedFunction } from 'vitest'
import { createViewTypeIpc } from './arrow-ipc-fixtures'

// Mock only the API layer, not apache-arrow
vi.mock('@/lib/api', () => ({
  authenticatedFetch: vi.fn(),
  AuthenticationError: class AuthenticationError extends Error {
    constructor() {
      super('Authentication required')
      this.name = 'AuthenticationError'
    }
  },
  getApiBase: () => '/api',
  getAuthBase: () => '',
}))

const mockedFetch = authenticatedFetch as MockedFunction<typeof authenticatedFetch>

// Helper to create a mock ReadableStream from chunks
function createMockStream(chunks: Uint8Array[]): ReadableStream<Uint8Array> {
  let index = 0
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      if (index < chunks.length) {
        controller.enqueue(chunks[index++])
      } else {
        controller.close()
      }
    },
  })
}

// Helper to create a mock Response with streaming body
function createMockResponse(chunks: Uint8Array[]): Response {
  return {
    ok: true,
    status: 200,
    statusText: 'OK',
    body: createMockStream(chunks),
    headers: new Headers(),
  } as Response
}

/** Splits a byte array into small chunks to simulate network chunking across message boundaries. */
function splitIntoSmallChunks(bytes: Uint8Array, size: number): Uint8Array[] {
  const out: Uint8Array[] = []
  for (let i = 0; i < bytes.length; i += size) {
    out.push(bytes.slice(i, Math.min(i + size, bytes.length)))
  }
  return out
}

describe.each([
  { label: 'uncompressed', compressed: false },
  { label: 'LZ4_FRAME-compressed', compressed: true },
])('streamQuery with Utf8View/BinaryView columns ($label)', ({ compressed }) => {
  beforeEach(() => {
    vi.resetAllMocks()
  })

  it('decodes the schema with Utf8View/BinaryView fields and round-trips values', async () => {
    const { chunks } = createViewTypeIpc({ compressed })

    mockedFetch.mockResolvedValue(createMockResponse(chunks))

    const results: StreamResult[] = []
    for await (const result of streamQuery({ sql: 'SELECT name, data FROM t' })) {
      results.push(result)
    }

    const schemaResult = results.find((r) => r.type === 'schema')
    const batchResults = results.filter((r) => r.type === 'batch')
    const doneResult = results.find((r) => r.type === 'done')

    expect(batchResults.length).toBeGreaterThan(0)
    expect(doneResult).toBeDefined()

    if (schemaResult?.type === 'schema') {
      const nameField = schemaResult.schema.fields.find((f) => f.name === 'name')!
      const dataField = schemaResult.schema.fields.find((f) => f.name === 'data')!
      expect(DataType.isUtf8View(nameField.type)).toBe(true)
      expect(DataType.isBinaryView(dataField.type)).toBe(true)
    }

    const names: (string | null)[] = []
    const dataValues: (Uint8Array | null)[] = []
    for (const result of batchResults) {
      if (result.type !== 'batch') continue
      const nameCol = result.batch.getChild('name')
      const dataCol = result.batch.getChild('data')
      for (let i = 0; i < result.batch.numRows; i++) {
        names.push(nameCol ? (nameCol.get(i) as string | null) : null)
        dataValues.push(dataCol ? (dataCol.get(i) as Uint8Array | null) : null)
      }
    }

    expect(names).toEqual(['short', 'this string is well over twelve bytes long', null])
    expect(Array.from(dataValues[0] as Uint8Array)).toEqual([97, 98, 99])
    expect(new TextDecoder().decode(dataValues[1] as Uint8Array)).toBe(
      'this binary value is well over twelve bytes long'
    )
    expect(dataValues[2]).toBeNull()
  })

  it('decodes correctly when the framed bytes are split across arbitrary chunk boundaries', async () => {
    const { chunks } = createViewTypeIpc({ compressed })

    const totalLength = chunks.reduce((sum, c) => sum + c.length, 0)
    const combined = new Uint8Array(totalLength)
    let offset = 0
    for (const chunk of chunks) {
      combined.set(chunk, offset)
      offset += chunk.length
    }
    const smallChunks = splitIntoSmallChunks(combined, 37)

    mockedFetch.mockResolvedValue(createMockResponse(smallChunks))

    const results: StreamResult[] = []
    for await (const result of streamQuery({ sql: 'SELECT name, data FROM t' })) {
      results.push(result)
    }

    const batchResults = results.filter((r) => r.type === 'batch')
    const names: (string | null)[] = []
    for (const result of batchResults) {
      if (result.type !== 'batch') continue
      const nameCol = result.batch.getChild('name')
      for (let i = 0; i < result.batch.numRows; i++) {
        names.push(nameCol ? (nameCol.get(i) as string | null) : null)
      }
    }

    expect(names).toEqual(['short', 'this string is well over twelve bytes long', null])
  })
})
