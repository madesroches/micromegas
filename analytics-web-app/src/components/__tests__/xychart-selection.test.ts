import { describe, it, expect } from 'vitest'
import {
  hitTestBars,
  findNearestPoint,
  isClick,
  nextSelection,
  sameHit,
  fadeCss,
  fadeStops,
  barColors,
  resolveSelection,
  type BarRect,
} from '../xychart-selection'

describe('hitTestBars', () => {
  const rects: BarRect[] = [
    { seriesIndex: 0, dataIdx: 0, left: 10, top: 20, width: 10, height: 30 },
    { seriesIndex: 1, dataIdx: 0, left: 15, top: 30, width: 10, height: 20 },
  ]

  it('hits a rect', () => {
    expect(hitTestBars(rects, 12, 25)?.seriesIndex).toBe(0)
  })

  it('includes edges', () => {
    expect(hitTestBars(rects, 10, 20)?.seriesIndex).toBe(0)
    expect(hitTestBars(rects, 14, 50)?.seriesIndex).toBe(0)
  })

  it('misses outside', () => {
    expect(hitTestBars(rects, 5, 25)).toBeNull()
    expect(hitTestBars(rects, 12, 10)).toBeNull()
    expect(hitTestBars([], 1, 1)).toBeNull()
  })

  it('returns the topmost of overlapping rects', () => {
    expect(hitTestBars(rects, 17, 40)?.seriesIndex).toBe(1)
  })
})

describe('findNearestPoint', () => {
  const pts = [
    { px: 0, py: 0, id: 'a' },
    { px: 10, py: 0, id: 'b' },
  ]

  it('finds the nearest within radius', () => {
    expect(findNearestPoint(pts, 8, 0, 5)?.id).toBe('b')
  })

  it('returns null beyond radius', () => {
    expect(findNearestPoint(pts, 5, 20, 8)).toBeNull()
  })

  it('keeps the first on a tie', () => {
    expect(findNearestPoint(pts, 5, 0, 8)?.id).toBe('a')
  })

  it('returns null for no candidates', () => {
    expect(findNearestPoint([], 0, 0, 8)).toBeNull()
  })
})

describe('isClick', () => {
  it('is a click below the threshold', () => {
    expect(isClick({ x: 0, y: 0 }, { x: 1, y: 1 }, 4)).toBe(true)
  })

  it('is a drag at or above the threshold', () => {
    expect(isClick({ x: 0, y: 0 }, { x: 10, y: 0 }, 4)).toBe(false)
    expect(isClick({ x: 0, y: 0 }, { x: 4, y: 0 }, 4)).toBe(false)
  })
})

describe('nextSelection', () => {
  const a = { seriesIndex: 0, rowIndex: 1 }
  const b = { seriesIndex: 0, rowIndex: 2 }

  it('selects', () => expect(nextSelection(null, a)).toEqual(a))
  it('clears on re-click', () => expect(nextSelection(a, { ...a })).toBeNull())
  it('switches', () => expect(nextSelection(a, b)).toEqual(b))
  it('clears on miss', () => expect(nextSelection(a, null)).toBeNull())
  it('treats other series as different', () => {
    expect(sameHit(a, { seriesIndex: 1, rowIndex: 1 })).toBe(false)
  })
})

describe('resolveSelection', () => {
  const rows = [[5, 3, null], [1, 2, 4]]

  it('maps a row to its data index', () => {
    expect(resolveSelection({ seriesIndex: 0, rowIndex: 3 }, rows)).toEqual({ seriesIndex: 0, dataIdx: 1 })
  })

  it('treats stale selections as none', () => {
    expect(resolveSelection({ seriesIndex: 0, rowIndex: 9 }, rows)).toBeNull()
    expect(resolveSelection({ seriesIndex: 5, rowIndex: 0 }, rows)).toBeNull()
    expect(resolveSelection(null, rows)).toBeNull()
  })
})

describe('fadeCss', () => {
  it('fades #rrggbb', () => {
    expect(fadeCss('#ff8000', 0.5)).toBe('rgba(255, 128, 0, 0.5)')
  })

  it('multiplies the existing alpha of #rrggbbaa', () => {
    expect(fadeCss('#ff000080', 0.5)).toBe(`rgba(255, 0, 0, ${(128 / 255) * 0.5})`)
  })

  it('multiplies the alpha of rgba()', () => {
    expect(fadeCss('rgba(1, 2, 3, 0.6)', 0.5)).toBe('rgba(1, 2, 3, 0.3)')
  })

  it('returns the same color for factor 1', () => {
    expect(fadeCss('#abcdef', 1)).toBe('#abcdef')
  })
})

describe('fadeStops', () => {
  it('fades colors and keeps offsets', () => {
    const out = fadeStops([{ x: 0.25, color: '#ff0000' }, { x: 1, color: 'rgba(0, 0, 0, 1)' }], 0.5)
    expect(out.map((s) => s.x)).toEqual([0.25, 1])
    expect(out[0].color).toBe('rgba(255, 0, 0, 0.5)')
    expect(out[1].color).toBe('rgba(0, 0, 0, 0.5)')
  })
})

describe('barColors', () => {
  const base = ['#ff0000', '#00ff00', '#0000ff']

  it('keeps base colors without a selection', () => {
    expect(barColors(base, null, 0, null)).toBe(base)
  })

  it('fades all but the selected bar', () => {
    const out = barColors(base, 1, 0, 0, 0.5)
    expect(out[1]).toBe('#00ff00')
    expect(out[0]).toBe('rgba(255, 0, 0, 0.5)')
    expect(out[2]).toBe('rgba(0, 0, 255, 0.5)')
  })

  it('fades every bar when the selection is in another series', () => {
    const out = barColors(base, 1, 1, 0, 0.5)
    expect(out.every((c) => c.startsWith('rgba('))).toBe(true)
  })
})
