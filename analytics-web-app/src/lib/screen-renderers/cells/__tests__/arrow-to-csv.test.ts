import { tableFromArrays } from 'apache-arrow'
import { arrowTableToCsv } from '../arrow-to-csv'

describe('arrowTableToCsv', () => {
  it('converts a simple table to CSV', () => {
    const table = tableFromArrays({
      name: ['Alice', 'Bob'],
      age: new Int32Array([30, 25]),
    })
    const csv = arrowTableToCsv(table)
    expect(csv).toBe('name,age\nAlice,30\nBob,25')
  })

  it('returns only headers for empty table (0 rows)', () => {
    const table = tableFromArrays({
      x: new Float64Array(0),
      y: new Float64Array(0),
    })
    const csv = arrowTableToCsv(table)
    expect(csv).toBe('x,y')
  })

  it('escapes values with commas, quotes, and newlines', () => {
    const table = tableFromArrays({
      text: ['hello, world', 'say "hi"', 'line1\nline2'],
    })
    const csv = arrowTableToCsv(table)
    const lines = csv.split('\n')
    expect(lines[0]).toBe('text')
    expect(lines[1]).toBe('"hello, world"')
    expect(lines[2]).toBe('"say ""hi"""')
    // The value with a newline gets quoted, so it spans lines 3-4
    expect(csv).toContain('"line1\nline2"')
  })

  it('converts null values to empty strings', () => {
    // tableFromArrays with nullable arrays
    const table = tableFromArrays({
      col: ['a', null, 'c'],
    })
    const csv = arrowTableToCsv(table)
    expect(csv).toBe('col\na\n\nc')
  })
})
