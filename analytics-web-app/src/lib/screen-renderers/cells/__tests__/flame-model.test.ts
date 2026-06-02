import { tableFromArrays } from 'apache-arrow'
import {
  buildFlameIndex,
  formatDuration,
  hitTest,
  laneYOffset,
  spanColor,
  spanColorIndex,
  totalHeight,
  type LaneIndex,
} from '../flame-model'

// A bits-mode (numeric begin/end) net-spans-style table:
//   row0 depth0 [0,24], row1 depth1 [0,8], row2 depth1 [8,24]
function makeBitsIndex() {
  const table = tableFromArrays({
    id: new BigInt64Array([1n, 2n, 3n]),
    parent: new BigInt64Array([0n, 1n, 1n]),
    name: ['root', 'childA', 'childB'],
    depth: new Int32Array([0, 1, 1]),
    begin: new BigInt64Array([0n, 0n, 8n]),
    end: new BigInt64Array([24n, 8n, 24n]),
  })
  return buildFlameIndex(table)
}

describe('spanColorIndex / spanColor', () => {
  it('is deterministic and within palette bounds', () => {
    const a = spanColorIndex('frame_update')
    const b = spanColorIndex('frame_update')
    expect(a).toBe(b)
    expect(a).toBeGreaterThanOrEqual(0)
    expect(a).toBeLessThan(15)
  })

  it('returns a hex color and a light-text flag for blue-family entries', () => {
    const [hex, textLight] = spanColor('anything')
    expect(hex).toMatch(/^#[0-9a-f]{6}$/i)
    expect(typeof textLight).toBe('boolean')
  })
})

describe('formatDuration', () => {
  it('renders nanoseconds, microseconds, milliseconds, seconds, minutes, and hours', () => {
    expect(formatDuration(0.0005)).toBe('500 ns')
    expect(formatDuration(0.5)).toBe('500 µs')
    expect(formatDuration(12.34)).toBe('12.3 ms')
    expect(formatDuration(2500)).toBe('2.50 s')
    expect(formatDuration(120_000)).toBe('2.00 min')
    expect(formatDuration(7_200_000)).toBe('2.00 h')
  })
})

describe('laneYOffset / totalHeight', () => {
  const lanes = [
    { id: 'a', name: 'a', maxDepth: 1, rowIndices: new Int32Array(), visualDepths: new Int32Array() },
    { id: 'b', name: 'b', maxDepth: 0, rowIndices: new Int32Array(), visualDepths: new Int32Array() },
  ] as LaneIndex[]

  it('offsets the first lane at 0 and stacks subsequent lanes', () => {
    expect(laneYOffset(lanes, 0)).toBe(0)
    // lane 0: header(24) + (1+1)*(20+1) + padding(4) = 24 + 42 + 4 = 70
    expect(laneYOffset(lanes, 1)).toBe(70)
  })

  it('totalHeight includes the last lane block', () => {
    // 70 + header(24) + (0+1)*(21) + padding(4) = 70 + 24 + 21 + 4 = 119
    expect(totalHeight(lanes)).toBe(119)
    expect(totalHeight([])).toBe(0)
  })
})

describe('hitTest', () => {
  const index = makeBitsIndex()

  it('returns null above the first lane', () => {
    expect(hitTest(index, 4, 10)).toBeNull()
  })

  it('hits the depth-0 root span', () => {
    // depth-0 band starts at laneTop=24; dataY=30 -> depth 0
    const hit = hitTest(index, 4, 30)
    expect(hit).not.toBeNull()
    expect(hit!.rowIndex).toBe(0)
  })

  it('discriminates between depth-1 siblings by x position', () => {
    // depth-1 band: relY in [24,45) maps to depth 1 (24 + 26 = 50)
    expect(hitTest(index, 4, 50)!.rowIndex).toBe(1)
    expect(hitTest(index, 20, 50)!.rowIndex).toBe(2)
  })

  it('returns null when x is past all spans at that depth', () => {
    expect(hitTest(index, 999, 30)).toBeNull()
  })
})
