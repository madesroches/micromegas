import { tableFromArrays } from 'apache-arrow'
import { buildFlameIndex, formatBits } from '../FlameGraphCell'

describe('formatBits', () => {
  it('formats small values in bits', () => {
    expect(formatBits(0)).toBe('0 b')
    expect(formatBits(42)).toBe('42 b')
    expect(formatBits(999)).toBe('999 b')
  })

  it('switches to Kb in the thousand-to-million range', () => {
    expect(formatBits(1_000)).toBe('1.0 Kb')
    expect(formatBits(4_500)).toBe('4.5 Kb')
    expect(formatBits(999_000)).toBe('999.0 Kb')
  })

  it('switches to Mb in the million-to-billion range', () => {
    expect(formatBits(1_000_000)).toBe('1.0 Mb')
    expect(formatBits(2_100_000)).toBe('2.1 Mb')
  })

  it('switches to Gb above a billion', () => {
    expect(formatBits(1_500_000_000)).toBe('1.5 Gb')
  })
})

describe('buildFlameIndex with Int64 begin/end (bits mode)', () => {
  it('indexes a net-spans-style table with numeric begin/end', () => {
    // Shape of a net_spans query: id/parent/name/depth/begin/end (all numeric).
    // tableFromArrays with BigInt64Array gives us Int64 columns.
    const table = tableFromArrays({
      id: new BigInt64Array([1n, 2n, 3n]),
      parent: new BigInt64Array([0n, 1n, 1n]),
      name: ['connection', 'propA', 'propB'],
      depth: new Int32Array([0, 1, 1]),
      begin: new BigInt64Array([0n, 0n, 8n]),
      end: new BigInt64Array([24n, 8n, 24n]),
    })

    const index = buildFlameIndex(table)
    expect(index.error).toBeUndefined()
    expect(index.xAxisMode).toBe('bits')
    expect(index.timeRange.min).toBe(0)
    expect(index.timeRange.max).toBe(24)
    expect(index.lanes.length).toBe(1)
    expect(index.lanes[0].rowIndices.length).toBe(3)
    // Max depth is 1 since we have root (depth=0) and two children (depth=1).
    expect(index.lanes[0].maxDepth).toBe(1)
  })
})
