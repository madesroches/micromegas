import { csvToArrowIPC } from '../cells/csv-to-arrow'
import { buildSortedIndices } from '../cells/ReferenceTableCell'

describe('buildSortedIndices', () => {
  it('returns identity indices when no sort column', () => {
    const { table } = csvToArrowIPC('name,val\nalice,3\nbob,1\ncharlie,2')
    const indices = buildSortedIndices(table, undefined, undefined)
    expect(indices).toEqual([0, 1, 2])
  })

  it('returns identity indices when sort column does not exist', () => {
    const { table } = csvToArrowIPC('name,val\nalice,3\nbob,1\ncharlie,2')
    const indices = buildSortedIndices(table, 'nonexistent', 'asc')
    expect(indices).toEqual([0, 1, 2])
  })

  it('sorts numeric column ascending', () => {
    const { table } = csvToArrowIPC('name,val\nalice,3\nbob,1\ncharlie,2')
    const indices = buildSortedIndices(table, 'val', 'asc')
    // val: 3(0), 1(1), 2(2) → sorted asc: 1(1), 2(2), 3(0)
    expect(indices).toEqual([1, 2, 0])
    expect(table.get(indices[0])!['val']).toBe(1)
    expect(table.get(indices[1])!['val']).toBe(2)
    expect(table.get(indices[2])!['val']).toBe(3)
  })

  it('sorts numeric column descending', () => {
    const { table } = csvToArrowIPC('name,val\nalice,3\nbob,1\ncharlie,2')
    const indices = buildSortedIndices(table, 'val', 'desc')
    // val: 3(0), 1(1), 2(2) → sorted desc: 3(0), 2(2), 1(1)
    expect(indices).toEqual([0, 2, 1])
    expect(table.get(indices[0])!['val']).toBe(3)
    expect(table.get(indices[1])!['val']).toBe(2)
    expect(table.get(indices[2])!['val']).toBe(1)
  })

  it('sorts string column ascending', () => {
    const { table } = csvToArrowIPC('name,tag\ncharlie,x\nalice,x\nbob,x')
    const indices = buildSortedIndices(table, 'name', 'asc')
    // name: charlie(0), alice(1), bob(2) → sorted asc: alice(1), bob(2), charlie(0)
    expect(indices).toEqual([1, 2, 0])
  })

  it('sorts string column descending', () => {
    const { table } = csvToArrowIPC('name,tag\ncharlie,x\nalice,x\nbob,x')
    const indices = buildSortedIndices(table, 'name', 'desc')
    // name: charlie(0), alice(1), bob(2) → sorted desc: charlie(0), bob(2), alice(1)
    expect(indices).toEqual([0, 2, 1])
  })

  it('handles NaN values in numeric columns (pushed to end)', () => {
    const { table } = csvToArrowIPC('val\n3\n\n1')
    // val: 3(0), NaN(1), 1(2)
    const indices = buildSortedIndices(table, 'val', 'asc')
    // NaN compares as null-ish, pushed to end: 1(2), 3(0), NaN(1)
    expect(table.get(indices[0])!['val']).toBe(1)
    expect(table.get(indices[1])!['val']).toBe(3)
    // last value is NaN
    expect(table.get(indices[2])!['val']).toBeNaN()
  })

  it('is stable: equal values preserve original order', () => {
    const { table } = csvToArrowIPC('name,group\nalice,A\nbob,A\ncharlie,B\ndave,A')
    const indices = buildSortedIndices(table, 'group', 'asc')
    // group A indices should maintain relative order: alice(0), bob(1), dave(3)
    const groupA = indices.filter((i) => table.get(i)!['group'] === 'A')
    expect(groupA).toEqual([0, 1, 3])
  })
})
