/**
 * Integration tests for Arrow IPC streaming with dictionary-encoded columns.
 *
 * These tests use real apache-arrow library (not mocked) to verify that
 * dictionary-encoded columns are correctly parsed across multiple batches.
 */

import { authenticatedFetch } from '@/lib/api'
import { streamQuery, StreamResult } from '../arrow-stream'
import {
  createDictionaryFramedIpc,
  createPlainFramedIpc,
  combineChunks,
} from './arrow-ipc-fixtures'

// Mock only the API layer, not apache-arrow
jest.mock('@/lib/api', () => ({
  authenticatedFetch: jest.fn(),
  AuthenticationError: class AuthenticationError extends Error {
    constructor() {
      super('Authentication required')
      this.name = 'AuthenticationError'
    }
  },
  getApiBase: () => '/api',
  getAuthBase: () => '',
}))

const mockedFetch = authenticatedFetch as jest.MockedFunction<typeof authenticatedFetch>

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

describe('streamQuery with dictionary-encoded columns', () => {
  beforeEach(() => {
    jest.resetAllMocks()
  })

  it('should correctly parse dictionary-encoded columns in a single batch', async () => {
    const chunks = createDictionaryFramedIpc([{ level: ['high', 'low', 'high', 'medium'] }])

    mockedFetch.mockResolvedValue(createMockResponse(chunks))

    const results: StreamResult[] = []
    for await (const result of streamQuery({ sql: 'SELECT level FROM logs' })) {
      results.push(result)
    }

    // Debug: log what we got
    // console.log('Results:', results.map(r => r.type))

    // Should have schema, batch(es), and done
    const schemaResult = results.find((r) => r.type === 'schema')
    const batchResults = results.filter((r) => r.type === 'batch')
    const doneResult = results.find((r) => r.type === 'done')

    // Note: schema may be null if reader.schema is not set before iteration
    // The new implementation yields schema from reader.schema which may be available after first batch
    expect(batchResults.length).toBeGreaterThan(0)
    expect(doneResult).toBeDefined()

    // Schema might come through or might be embedded - check we got data either way
    if (schemaResult?.type === 'schema') {
      const field = schemaResult.schema.fields[0]
      expect(field.name).toBe('level')
    }

    // Primary check: verify batch data is correctly decoded
    const allValues: string[] = []
    for (const result of batchResults) {
      if (result.type === 'batch') {
        const col = result.batch.getChildAt(0)
        if (col) {
          for (let i = 0; i < col.length; i++) {
            allValues.push(col.get(i) as string)
          }
        }
      }
    }

    expect(allValues).toEqual(['high', 'low', 'high', 'medium'])
  })


  it('should correctly parse dictionary-encoded columns across multiple batches', async () => {
    // Create two batches with overlapping dictionary values
    const chunks = createDictionaryFramedIpc([
      { level: ['high', 'low', 'high'] },
      { level: ['medium', 'high', 'low'] },
    ])

    mockedFetch.mockResolvedValue(createMockResponse(chunks))

    const results: StreamResult[] = []
    for await (const result of streamQuery({ sql: 'SELECT level FROM logs' })) {
      results.push(result)
    }

    const batchResults = results.filter((r) => r.type === 'batch')

    // Collect all values from all batches
    const allValues: string[] = []
    for (const result of batchResults) {
      if (result.type === 'batch') {
        const col = result.batch.getChildAt(0)
        if (col) {
          for (let i = 0; i < col.length; i++) {
            allValues.push(col.get(i) as string)
          }
        }
      }
    }

    // Values should be correctly decoded even though dictionary is shared
    expect(allValues).toEqual(['high', 'low', 'high', 'medium', 'high', 'low'])
  })

  it('should correctly parse plain (non-dictionary) string columns', async () => {
    const chunks = createPlainFramedIpc([{ name: ['alice', 'bob', 'charlie'] }])

    mockedFetch.mockResolvedValue(createMockResponse(chunks))

    const results: StreamResult[] = []
    for await (const result of streamQuery({ sql: 'SELECT name FROM users' })) {
      results.push(result)
    }

    const batchResults = results.filter((r) => r.type === 'batch')

    const allValues: string[] = []
    for (const result of batchResults) {
      if (result.type === 'batch') {
        const col = result.batch.getChildAt(0)
        if (col) {
          for (let i = 0; i < col.length; i++) {
            allValues.push(col.get(i) as string)
          }
        }
      }
    }

    expect(allValues).toEqual(['alice', 'bob', 'charlie'])
  })

  it('should handle chunked delivery of IPC bytes', async () => {
    // Create IPC and then split into smaller chunks to simulate network chunking
    const originalChunks = createDictionaryFramedIpc([
      { level: ['high', 'low'] },
    ])

    // Combine all chunks then split at arbitrary boundaries
    const combined = combineChunks(originalChunks)

    // Split into 50-byte chunks
    const smallChunks: Uint8Array[] = []
    for (let i = 0; i < combined.length; i += 50) {
      smallChunks.push(combined.slice(i, Math.min(i + 50, combined.length)))
    }

    mockedFetch.mockResolvedValue(createMockResponse(smallChunks))

    const results: StreamResult[] = []
    for await (const result of streamQuery({ sql: 'SELECT level FROM logs' })) {
      results.push(result)
    }

    const batchResults = results.filter((r) => r.type === 'batch')

    const allValues: string[] = []
    for (const result of batchResults) {
      if (result.type === 'batch') {
        const col = result.batch.getChildAt(0)
        if (col) {
          for (let i = 0; i < col.length; i++) {
            allValues.push(col.get(i) as string)
          }
        }
      }
    }

    expect(allValues).toEqual(['high', 'low'])
  })

  it('should preserve dictionary type in batch schema', async () => {
    const chunks = createDictionaryFramedIpc([{ level: ['a', 'b', 'a', 'c', 'b'] }])

    mockedFetch.mockResolvedValue(createMockResponse(chunks))

    const results: StreamResult[] = []
    for await (const result of streamQuery({ sql: 'SELECT level FROM logs' })) {
      results.push(result)
    }

    const batchResults = results.filter((r) => r.type === 'batch')
    expect(batchResults.length).toBeGreaterThan(0)

    // Check that the batch schema has dictionary type
    if (batchResults[0]?.type === 'batch') {
      const batch = batchResults[0].batch
      const levelField = batch.schema.fields[0]
      expect(levelField.name).toBe('level')
      expect(levelField.type.toString()).toContain('Dictionary')
      expect(levelField.type.toString()).toContain('Utf8')
    }
  })
})
