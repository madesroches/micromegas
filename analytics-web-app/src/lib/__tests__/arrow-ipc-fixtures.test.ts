import { createDictionaryFramedIpc, createPlainFramedIpc, combineChunks } from './arrow-ipc-fixtures'

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
