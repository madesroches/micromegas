import { tableFromArrays } from 'apache-arrow'
import {
  buildFlameIndex,
  clampTooltipPosition,
  fitLabelText,
  formatDuration,
  hitTest,
  labelClipRect,
  laneYOffset,
  spanColor,
  spanColorIndex,
  totalHeight,
  truncateSpanName,
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

describe('labelClipRect', () => {
  it('insets both edges for a normal on-screen span', () => {
    expect(labelClipRect(100, 300, 800)).toEqual({ left: 102, width: 196 })
  })

  it('tracks the true right edge for an off-screen-left span', () => {
    expect(labelClipRect(-500, 200, 800)).toEqual({ left: 0, width: 198 })
  })

  it('clamps to the canvas but never extends past the right edge for a wider-than-canvas span', () => {
    expect(labelClipRect(-100, 5000, 800)).toEqual({ left: 0, width: 800 })
  })

  it('returns width 0 for a span whose visible width is below the inset', () => {
    expect(labelClipRect(0, 3, 800)).toEqual({ left: 2, width: 0 })
  })
})

describe('fitLabelText', () => {
  it('returns a name unchanged when it fits', () => {
    expect(fitLabelText('short', 100, 10)).toBe('short')
  })

  it('truncates a too-long name with a trailing ellipsis', () => {
    expect(fitLabelText('abcdef', 30, 10)).toBe('ab…')
  })

  it('returns empty string when zero or negative width is available', () => {
    expect(fitLabelText('abcdef', 0, 10)).toBe('')
    expect(fitLabelText('abcdef', -5, 10)).toBe('')
  })

  it('returns the name unchanged when charWidth is not positive', () => {
    expect(fitLabelText('abcdef', 100, 0)).toBe('abcdef')
  })
})

describe('truncateSpanName', () => {
  it('returns a short name unchanged', () => {
    expect(truncateSpanName('short name')).toBe('short name')
  })

  it('caps a long name to max chars plus an ellipsis', () => {
    const long = 'x'.repeat(400)
    const result = truncateSpanName(long, 300)
    expect(result).toBe('x'.repeat(300) + '…')
  })

  it('preserves embedded newlines within the cap', () => {
    const withNewlines = 'line1\nline2\nline3'
    expect(truncateSpanName(withNewlines, 300)).toBe(withNewlines)
  })
})

describe('clampTooltipPosition', () => {
  it('positions below-right of the cursor when there is room', () => {
    expect(clampTooltipPosition(100, 100, 50, 30, 800, 600)).toEqual({ left: 112, top: 112 })
  })

  it('flips left when near the right edge, keeping the right edge in bounds', () => {
    const { left } = clampTooltipPosition(780, 100, 50, 30, 800, 600)
    expect(left + 50).toBeLessThanOrEqual(800)
  })

  it('flips above when near the bottom edge', () => {
    const { top } = clampTooltipPosition(100, 590, 50, 30, 800, 600)
    expect(top + 30).toBeLessThanOrEqual(600)
  })

  it('clamps to 0 rather than going negative when the box is larger than the container', () => {
    expect(clampTooltipPosition(10, 10, 900, 700, 800, 600)).toEqual({ left: 0, top: 0 })
  })
})
