import { createDictionaryFramedIpc, createPlainFramedIpc, createViewTypeIpc, combineChunks } from './arrow-ipc-fixtures'
import { tableFromIPC, DataType } from 'apache-arrow'

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

  // Whole-buffer decode path: covers useCellExecution.ts's `tableFromIPC(ipcBytes)`
  // call sites, which decode a complete IPC buffer directly rather than
  // streaming through RecordBatchReader (that path is covered separately in
  // arrow-stream-view-types.test.ts). Uncompressed matches the in-browser
  // datafusion-wasm output; LZ4-compressed matches fetchQueryIPC's collection
  // of the same server response stream_query.rs produces. Neither case can be
  // exercised through a mock — useCellExecution.test.ts mocks `apache-arrow`'s
  // `tableFromIPC` entirely, so these assertions must run against the real
  // library to mean anything about view-type decoding.
  describe('createViewTypeIpc', () => {
    describe.each([
      { label: 'uncompressed', compressed: false },
      { label: 'LZ4_FRAME-compressed', compressed: true },
    ])('$label whole IPC buffer', ({ compressed }) => {
      it('decodes a Utf8View/BinaryView table', () => {
        const { raw } = createViewTypeIpc({ compressed })

        const table = tableFromIPC(raw)

        const nameField = table.schema.fields.find((f) => f.name === 'name')!
        const dataField = table.schema.fields.find((f) => f.name === 'data')!
        expect(DataType.isUtf8View(nameField.type)).toBe(true)
        expect(DataType.isBinaryView(dataField.type)).toBe(true)

        expect(table.numRows).toBe(3)
        const row0 = table.get(0)!
        const row1 = table.get(1)!
        const row2 = table.get(2)!

        expect(row0.name).toBe('short')
        expect(row1.name).toBe('this string is well over twelve bytes long')
        expect(row2.name).toBeNull()

        expect(Array.from(row0.data as Uint8Array)).toEqual([97, 98, 99])
        expect(new TextDecoder().decode(row1.data as Uint8Array)).toBe(
          'this binary value is well over twelve bytes long'
        )
        expect(row2.data).toBeNull()
      })
    })
  })
})
