import { csvToArrowIPC } from '../cells/csv-to-arrow'
import { tableFromIPC, Float64, DataType } from 'apache-arrow'

describe('csvToArrowIPC', () => {
  it('parses basic CSV with headers and data rows', () => {
    const csv = 'name,value\nalice,100\nbob,200'
    const { table, ipcBytes } = csvToArrowIPC(csv)

    expect(table.numRows).toBe(2)
    expect(table.numCols).toBe(2)
    expect(table.schema.fields.map((f) => f.name)).toEqual(['name', 'value'])

    // Verify IPC bytes round-trip
    const restored = tableFromIPC(ipcBytes)
    expect(restored.numRows).toBe(2)
  })

  it('detects numeric columns as Float64', () => {
    const csv = 'id,score\n1,95.5\n2,87.3'
    const { table } = csvToArrowIPC(csv)

    expect(table.schema.fields[0].type).toBeInstanceOf(Float64)
    expect(table.schema.fields[1].type).toBeInstanceOf(Float64)

    const row0 = table.get(0)!
    expect(row0['id']).toBe(1)
    expect(row0['score']).toBe(95.5)
  })

  it('uses string type for mixed columns', () => {
    const csv = 'code,label\n100,active\n200,inactive'
    const { table } = csvToArrowIPC(csv)

    // 'code' is all numeric
    expect(table.schema.fields[0].type).toBeInstanceOf(Float64)
    // 'label' has non-numeric values (Arrow may dictionary-encode strings)
    expect(DataType.isUtf8(table.schema.fields[1].type) || DataType.isDictionary(table.schema.fields[1].type)).toBe(true)
  })

  it('treats column as string if any value is non-numeric', () => {
    const csv = 'val\n1\n2\nabc\n4'
    const { table } = csvToArrowIPC(csv)

    expect(DataType.isUtf8(table.schema.fields[0].type) || DataType.isDictionary(table.schema.fields[0].type)).toBe(true)
  })

  it('handles quoted fields with commas', () => {
    const csv = 'name,desc\nalice,"hello, world"\nbob,"foo, bar"'
    const { table } = csvToArrowIPC(csv)

    expect(table.numRows).toBe(2)
    const row0 = table.get(0)!
    expect(row0['desc']).toBe('hello, world')
  })

  it('handles escaped quotes in fields', () => {
    const csv = 'name,note\nalice,"she said ""hi"""\nbob,normal'
    const { table } = csvToArrowIPC(csv)

    const row0 = table.get(0)!
    expect(row0['note']).toBe('she said "hi"')
  })

  it('handles empty cells', () => {
    const csv = 'a,b\n1,\n,2'
    const { table } = csvToArrowIPC(csv)

    expect(table.numRows).toBe(2)
    // Column 'a' has a missing value (""), so it's mixed -> Utf8 or numeric with NaN
    // Column 'b' has a missing value (""), same
  })

  it('handles single column', () => {
    const csv = 'name\nalice\nbob'
    const { table } = csvToArrowIPC(csv)

    expect(table.numCols).toBe(1)
    expect(table.numRows).toBe(2)
  })

  it('handles trailing newlines', () => {
    const csv = 'a,b\n1,2\n3,4\n'
    const { table } = csvToArrowIPC(csv)

    expect(table.numRows).toBe(2)
  })

  it('throws on empty string', () => {
    expect(() => csvToArrowIPC('')).toThrow()
  })

  it('throws on headers only (no data rows)', () => {
    expect(() => csvToArrowIPC('a,b,c')).toThrow('at least one data row')
  })

  it('treats empty values as NaN for numeric columns', () => {
    const csv = 'val\n1\n\n3'
    const { table } = csvToArrowIPC(csv)

    expect(table.schema.fields[0].type).toBeInstanceOf(Float64)
    const row1 = table.get(1)!
    expect(row1['val']).toBeNaN()
  })
})
