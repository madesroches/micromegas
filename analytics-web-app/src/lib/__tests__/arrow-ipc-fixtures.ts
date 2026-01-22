/**
 * Test fixtures for Arrow IPC streaming with dictionary-encoded columns.
 *
 * This module generates real Arrow IPC bytes using apache-arrow library,
 * then frames them in the JSON protocol used by our backend:
 *   {"type":"schema","size":N}\n[schema bytes]
 *   {"type":"batch","size":N}\n[batch bytes]
 *   {"type":"done"}\n
 */

import {
  tableToIPC,
  tableFromArrays,
  vectorFromArray,
  Dictionary,
  Utf8,
  Int32,
  Table,
} from 'apache-arrow'

/**
 * Splits Arrow IPC stream bytes into individual messages.
 * Arrow IPC stream format: each message is prefixed with a 4-byte continuation
 * indicator (0xFFFFFFFF) followed by 4-byte metadata length.
 */
function splitIpcMessages(ipcBytes: Uint8Array): Uint8Array[] {
  const messages: Uint8Array[] = []
  let offset = 0
  const view = new DataView(ipcBytes.buffer, ipcBytes.byteOffset, ipcBytes.byteLength)

  while (offset < ipcBytes.length) {
    // Check for continuation indicator (0xFFFFFFFF in little-endian)
    const continuation = view.getInt32(offset, true)
    if (continuation !== -1) {
      // Not a continuation marker, might be end of stream
      break
    }
    offset += 4

    // Read metadata length
    const metadataLength = view.getInt32(offset, true)
    offset += 4

    if (metadataLength === 0) {
      // End of stream marker (EOS)
      break
    }

    // Metadata is padded to 8-byte alignment
    const paddedMetadataLength = (metadataLength + 7) & ~7

    // Read body length from flatbuffer metadata
    // The body length is at a specific offset in the Message flatbuffer
    // For simplicity, we'll scan for the message end by looking at the structure
    const messageStart = offset - 8 // Include continuation and length

    // Read the full message including body
    // Body length is encoded in the Message metadata
    const metadataEnd = offset + paddedMetadataLength

    // The body length is in the Message flatbuffer at offset 8 from metadata start
    // Message schema: { version: int, header_type: byte, header: union, bodyLength: long }
    // Actually, let's just compute the message size differently

    // For IPC stream, each message is:
    // - 4 bytes: continuation (0xFFFFFFFF)
    // - 4 bytes: metadata size
    // - metadata (padded to 8 bytes)
    // - body (size from metadata)

    // Read body length from the Message flatbuffer
    // The Message table has: version (4), header_type (1), header (4), bodyLength (8)
    // But it's a flatbuffer so we need to navigate the vtable...

    let nextOffset = metadataEnd

    // Scan forward to find next message or end
    while (nextOffset + 8 <= ipcBytes.length) {
      const nextCont = view.getInt32(nextOffset, true)
      const nextLen = view.getInt32(nextOffset + 4, true)
      if (nextCont === -1 && (nextLen === 0 || nextLen > 0)) {
        // Found next message or EOS
        break
      }
      nextOffset += 8 // Move in 8-byte increments (alignment)
    }

    if (nextOffset > ipcBytes.length) {
      nextOffset = ipcBytes.length
    }

    messages.push(ipcBytes.slice(messageStart, nextOffset))
    offset = nextOffset
  }

  return messages
}

/**
 * Creates a framed IPC stream with dictionary-encoded string columns.
 * Returns chunks that can be fed to a mock ReadableStream.
 */
export function createDictionaryFramedIpc(
  batches: Array<{ level: string[] }>
): Uint8Array[] {
  const chunks: Uint8Array[] = []
  const encoder = new TextEncoder()

  // Create dictionary type
  const dictType = new Dictionary(new Utf8(), new Int32())

  // Build tables for each batch
  const tables: Table[] = batches.map((batch) => {
    const vec = vectorFromArray(batch.level, dictType)
    return tableFromArrays({ level: vec })
  })

  // Combine into single table and get IPC bytes
  let combined = tables[0]
  for (let i = 1; i < tables.length; i++) {
    combined = combined.concat(tables[i])
  }

  const ipcBytes = tableToIPC(combined, 'stream')

  // Split IPC into messages
  const messages = splitIpcMessages(ipcBytes)

  if (messages.length === 0) {
    throw new Error('No IPC messages generated')
  }

  // First message is schema
  const schemaFrame = `{"type":"schema","size":${messages[0].length}}\n`
  chunks.push(encoder.encode(schemaFrame))
  chunks.push(messages[0])

  // Remaining messages are batches (may include dictionary batches)
  for (let i = 1; i < messages.length; i++) {
    const batchFrame = `{"type":"batch","size":${messages[i].length}}\n`
    chunks.push(encoder.encode(batchFrame))
    chunks.push(messages[i])
  }

  // Done frame
  chunks.push(encoder.encode('{"type":"done"}\n'))

  return chunks
}

/**
 * Creates a simple framed IPC stream with plain string columns (no dictionary).
 */
export function createPlainFramedIpc(
  batches: Array<{ name: string[] }>
): Uint8Array[] {
  const chunks: Uint8Array[] = []
  const encoder = new TextEncoder()

  // Build tables for each batch (plain Utf8, no dictionary)
  const tables: Table[] = batches.map((batch) => {
    return tableFromArrays({ name: batch.name })
  })

  // Combine into single table
  let combined = tables[0]
  for (let i = 1; i < tables.length; i++) {
    combined = combined.concat(tables[i])
  }

  const ipcBytes = tableToIPC(combined, 'stream')
  const messages = splitIpcMessages(ipcBytes)

  if (messages.length === 0) {
    throw new Error('No IPC messages generated')
  }

  // Schema frame
  const schemaFrame = `{"type":"schema","size":${messages[0].length}}\n`
  chunks.push(encoder.encode(schemaFrame))
  chunks.push(messages[0])

  // Batch frames
  for (let i = 1; i < messages.length; i++) {
    const batchFrame = `{"type":"batch","size":${messages[i].length}}\n`
    chunks.push(encoder.encode(batchFrame))
    chunks.push(messages[i])
  }

  // Done frame
  chunks.push(encoder.encode('{"type":"done"}\n'))

  return chunks
}

/**
 * Combines multiple Uint8Arrays into chunks suitable for streaming.
 * Can optionally split at arbitrary boundaries to test chunking.
 */
export function combineChunks(chunks: Uint8Array[]): Uint8Array {
  const totalLength = chunks.reduce((sum, c) => sum + c.length, 0)
  const result = new Uint8Array(totalLength)
  let offset = 0
  for (const chunk of chunks) {
    result.set(chunk, offset)
    offset += chunk.length
  }
  return result
}

// Self-tests for the fixture generators
describe('arrow-ipc-fixtures', () => {
  describe('createDictionaryFramedIpc', () => {
    it('should create valid framed IPC with dictionary columns', () => {
      const chunks = createDictionaryFramedIpc([
        { level: ['high', 'low', 'high'] },
        { level: ['medium', 'high', 'low'] },
      ])

      // Should have schema frame, schema bytes, batch frames, batch bytes, done frame
      expect(chunks.length).toBeGreaterThanOrEqual(3)

      // First chunk should be schema frame JSON
      const schemaFrame = new TextDecoder().decode(chunks[0])
      expect(schemaFrame).toMatch(/^\{"type":"schema","size":\d+\}/)

      // Last chunk should be done frame
      const doneFrame = new TextDecoder().decode(chunks[chunks.length - 1])
      expect(doneFrame).toBe('{"type":"done"}\n')
    })

    it('should produce bytes that can be round-tripped through RecordBatchReader', async () => {
      const { RecordBatchReader } = await import('apache-arrow')

      const chunks = createDictionaryFramedIpc([
        { level: ['high', 'low', 'high'] },
      ])

      // Extract just the IPC bytes (skip JSON frames)
      const ipcChunks: Uint8Array[] = []
      for (let i = 0; i < chunks.length; i++) {
        const text = new TextDecoder().decode(chunks[i])
        // Skip JSON frame lines, keep binary data
        if (!text.startsWith('{')) {
          ipcChunks.push(chunks[i])
        }
      }

      const combined = combineChunks(ipcChunks)
      const reader = await RecordBatchReader.from(combined)

      const batches = []
      for await (const batch of reader) {
        batches.push(batch)
      }

      expect(batches.length).toBeGreaterThan(0)
      const totalRows = batches.reduce((sum, b) => sum + b.numRows, 0)
      expect(totalRows).toBe(3)
    })
  })

  describe('createPlainFramedIpc', () => {
    it('should create valid framed IPC with plain string columns', () => {
      const chunks = createPlainFramedIpc([{ name: ['alice', 'bob', 'charlie'] }])

      expect(chunks.length).toBeGreaterThanOrEqual(3)

      const schemaFrame = new TextDecoder().decode(chunks[0])
      expect(schemaFrame).toMatch(/^\{"type":"schema","size":\d+\}/)

      const doneFrame = new TextDecoder().decode(chunks[chunks.length - 1])
      expect(doneFrame).toBe('{"type":"done"}\n')
    })
  })
})
