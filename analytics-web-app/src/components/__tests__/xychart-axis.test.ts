import type uPlot from 'uplot'
import {
  buildXAxisConfig,
  buildXScale,
  buildYAxisConfig,
  formatYAxisTick,
  yAxisSize,
  estimateLabelWidth,
  AVG_CHAR_WIDTH_PX,
  ROTATE_DEG,
  BASE_SIZE,
  BASE_MIN_SPACE_PX,
  ROTATED_MIN_SPACE_PX,
  MAX_ROTATED_SIZE,
  ROTATED_SIZE_FRACTION,
  DEFAULT_RIGHT_CUSHION_PX,
  Y_AXIS_BASE_SIZE_PX,
  Y_AXIS_MAX_SIZE_PX,
  Y_AXIS_SIZE_FRACTION,
  AXIS_CHROME_PX,
  TICK_LABEL_PADDING_PX,
} from '../xychart-axis'

// The `values` formatter ignores its uPlot argument; pass a stub.
const u = undefined as unknown as uPlot

describe('buildXAxisConfig', () => {
  it('time mode leaves values/incrs unset (uPlot uses its time defaults)', () => {
    const { axis, rightPadding } = buildXAxisConfig('time')
    expect(axis.values).toBeUndefined()
    expect(axis.incrs).toBeUndefined()
    expect(axis.size).toBe(65)
    expect(axis.rotate).toBeUndefined()
    expect(rightPadding).toBeNull()
  })

  it('categorical mode maps tick indices to labels and blanks out-of-range', () => {
    const { axis } = buildXAxisConfig('categorical', ['a', 'b', 'c'])
    expect(axis.incrs).toEqual([1])
    const fn = axis.values as (u: uPlot, vals: number[]) => string[]
    expect(fn(u, [0, 1, 2, 3])).toEqual(['a', 'b', 'c', ''])
    // Rounds fractional tick positions to the nearest index.
    expect(fn(u, [1.4])).toEqual(['b'])
  })

  it('categorical without labels falls through to the default (no values)', () => {
    const { axis } = buildXAxisConfig('categorical')
    expect(axis.values).toBeUndefined()
  })

  it('numeric mode abbreviates with magnitude-dependent precision', () => {
    const { axis, rightPadding } = buildXAxisConfig('numeric')
    const fn = axis.values as (u: uPlot, vals: number[]) => string[]
    expect(fn(u, [0])).toEqual(['0'])
    expect(fn(u, [12345])).toEqual([(12345).toLocaleString()])
    expect(fn(u, [3.14159])).toEqual(['3.1'])
    expect(fn(u, [0.0123])).toEqual([(0.0123).toPrecision(2)])
    expect(axis.rotate).toBeUndefined()
    expect(axis.space).toBe(60)
    expect(rightPadding).toBeNull()
  })

  describe('adaptive rotation (categorical mode)', () => {
    it('estimateLabelWidth returns label.length * AVG_CHAR_WIDTH_PX', () => {
      expect(estimateLabelWidth('abcd')).toBe(4 * AVG_CHAR_WIDTH_PX)
      expect(estimateLabelWidth('')).toBe(0)
    })

    it('rotate() returns 0 for short labels with ample foundSpace', () => {
      const { axis } = buildXAxisConfig('categorical', ['a', 'b'])
      const rotate = axis.rotate as (u: uPlot, values: (string | number)[], axisIdx: number, foundSpace: number) => number
      expect(rotate(u, ['a', 'b'], 0, 60)).toBe(0)
    })

    it('rotate() returns ROTATE_DEG for a long label with narrow foundSpace', () => {
      const { axis } = buildXAxisConfig('categorical', ['long-label'])
      const rotate = axis.rotate as (u: uPlot, values: (string | number)[], axisIdx: number, foundSpace: number) => number
      const longLabel = 'a-very-long-category-label-string'
      expect(rotate(u, [longLabel], 0, 20)).toBe(ROTATE_DEG)
    })

    it('size() after a rotating rotate() call is > BASE_SIZE and capped by height', () => {
      const { axis } = buildXAxisConfig('categorical', ['x'])
      const rotate = axis.rotate as (u: uPlot, values: (string | number)[], axisIdx: number, foundSpace: number) => number
      const size = axis.size as (self: uPlot) => number
      const longLabel = 'a-very-long-category-label-string'
      rotate(u, [longLabel], 0, 20)
      const stubSelf = { height: 250 } as uPlot
      const result = size(stubSelf)
      expect(result).toBeGreaterThan(BASE_SIZE)
      expect(result).toBeLessThanOrEqual(Math.min(MAX_ROTATED_SIZE, Math.round(250 * ROTATED_SIZE_FRACTION)))
    })

    it('size() after a non-rotating rotate() call returns exactly BASE_SIZE', () => {
      const { axis } = buildXAxisConfig('categorical', ['a', 'b'])
      const rotate = axis.rotate as (u: uPlot, values: (string | number)[], axisIdx: number, foundSpace: number) => number
      const size = axis.size as (self: uPlot) => number
      rotate(u, ['a', 'b'], 0, 60)
      expect(size({ height: 250 } as uPlot)).toBe(BASE_SIZE)
    })

    it('size() called with no arguments at all (uPlot init call shape) returns BASE_SIZE without throwing', () => {
      const { axis } = buildXAxisConfig('categorical', ['a', 'b'])
      const size = axis.size as (self?: uPlot) => number
      expect(() => size()).not.toThrow()
      expect(size()).toBe(BASE_SIZE)
    })

    it('space() reflects the previous rotate() decision: 60 before/non-rotating, 20 after rotating', () => {
      const { axis } = buildXAxisConfig('categorical', ['a', 'b'])
      const rotate = axis.rotate as (u: uPlot, values: (string | number)[], axisIdx: number, foundSpace: number) => number
      const space = axis.space as () => number
      expect(space()).toBe(BASE_MIN_SPACE_PX)
      rotate(u, ['a', 'b'], 0, 60)
      expect(space()).toBe(BASE_MIN_SPACE_PX)
      const longLabel = 'a-very-long-category-label-string'
      rotate(u, [longLabel], 0, 20)
      expect(space()).toBe(ROTATED_MIN_SPACE_PX)
    })

    it('rightPadding() before any rotate() call reproduces autoPadSide', () => {
      const { rightPadding } = buildXAxisConfig('categorical', ['a', 'b'])
      const fn = rightPadding as (self: uPlot, side: number, sidesWithAxes: [boolean, boolean, boolean, boolean], cycleNum: number) => number
      const stubSelf = { width: 800 } as uPlot
      expect(fn(stubSelf, 1, [false, false, true, true], 0)).toBe(DEFAULT_RIGHT_CUSHION_PX)
      expect(fn(stubSelf, 1, [false, true, true, true], 0)).toBe(0)
    })

    it('rightPadding() after a rotating rotate() call reserves capped width, reduced when a right axis exists', () => {
      const { axis, rightPadding } = buildXAxisConfig('categorical', ['a', 'b'])
      const rotate = axis.rotate as (u: uPlot, values: (string | number)[], axisIdx: number, foundSpace: number) => number
      const fn = rightPadding as (self: uPlot, side: number, sidesWithAxes: [boolean, boolean, boolean, boolean], cycleNum: number) => number
      const longLabel = 'a-very-long-category-label-string'
      rotate(u, [longLabel], 0, 20)
      const stubSelf = { width: 800 } as uPlot
      const cap = Math.min(MAX_ROTATED_SIZE, Math.round(800 * ROTATED_SIZE_FRACTION))
      const noRightAxis = fn(stubSelf, 1, [false, false, true, true], 0)
      expect(noRightAxis).toBeGreaterThan(DEFAULT_RIGHT_CUSHION_PX)
      expect(noRightAxis).toBeLessThanOrEqual(cap)
      const withRightAxis = fn(stubSelf, 1, [false, true, true, true], 0)
      expect(withRightAxis).toBe(Math.max(0, noRightAxis - Y_AXIS_BASE_SIZE_PX))
    })
  })
})

describe('buildXScale', () => {
  it('categorical mode pads the range by half a slot on each side', () => {
    const scale = buildXScale('categorical')
    expect(scale.range).toBeDefined()
    const fn = scale.range as (u: uPlot, dataMin: number, dataMax: number) => [number, number]
    expect(fn(u, 0, 3)).toEqual([-0.5, 3.5])
  })

  it('time and numeric modes leave range unset', () => {
    expect(buildXScale('time').range).toBeUndefined()
    expect(buildXScale('numeric').range).toBeUndefined()
  })
})

describe('formatYAxisTick', () => {
  it('formats plain values with magnitude-dependent precision and a unit suffix', () => {
    expect(formatYAxisTick(0, 1, 'ms', null)).toBe('0 ms')
    expect(formatYAxisTick(123.456, 1, 'ms', null)).toBe('123 ms')
    expect(formatYAxisTick(12.345, 1, 'ms', null)).toBe('12.3 ms')
    expect(formatYAxisTick(1.2345, 1, 'ms', null)).toBe('1.23 ms')
    expect(formatYAxisTick(0.012345, 1, 'ms', null)).toBe((0.012345).toPrecision(2) + ' ms')
  })

  it('applies the axis conversion factor before formatting', () => {
    expect(formatYAxisTick(1_500_000, 0.001, 'ms', null)).toBe('1500 ms')
  })

  it('formats currency scales via Intl and ignores the unit suffix/conversion factor', () => {
    const expected = new Intl.NumberFormat(undefined, { style: 'currency', currency: 'USD' }).format(1234.5)
    expect(formatYAxisTick(1234.5, 1, 'USD', 'USD')).toBe(expected)
  })

  it('formats a second currency correctly', () => {
    const expected = new Intl.NumberFormat(undefined, { style: 'currency', currency: 'CAD' }).format(50)
    expect(formatYAxisTick(50, 1, 'CAD', 'CAD')).toBe(expected)
  })

  it('produces no trailing space with an empty (dimensionless) display unit', () => {
    expect(formatYAxisTick(100, 1, '', null)).toBe('100')
    expect(formatYAxisTick(0, 1, '', null)).toBe('0')
  })

  it('attaches a /s display unit without a leading space', () => {
    expect(formatYAxisTick(100, 1, '/s', null)).toBe('100/s')
  })

  it('attaches °, °C, and % display units without a space', () => {
    expect(formatYAxisTick(180, 1, '°', null)).toBe('180°')
    expect(formatYAxisTick(210, 1, '°C', null)).toBe('210°C')
    expect(formatYAxisTick(500, 1, '%', null)).toBe('500%')
  })
})

describe('yAxisSize', () => {
  it('returns the floor for the uPlot init call shape (values === null)', () => {
    expect(yAxisSize(660, null)).toBe(Y_AXIS_BASE_SIZE_PX)
  })

  it('returns exactly the floor for short labels that already fit (grow-only regression guard)', () => {
    expect(yAxisSize(660, ['0', '100', '200'])).toBe(Y_AXIS_BASE_SIZE_PX)
  })

  it('grows past the floor for a long label, matching estimateLabelWidth + chrome + padding', () => {
    const label = '100 ops_per_sec'
    const result = yAxisSize(660, [label])
    expect(result).toBeGreaterThan(Y_AXIS_BASE_SIZE_PX)
    expect(result).toBe(estimateLabelWidth(label) + AXIS_CHROME_PX + TICK_LABEL_PADDING_PX)
  })

  it('tracks the widest label regardless of its position in the array', () => {
    const wide = '100 ops_per_sec'
    const expected = estimateLabelWidth(wide) + AXIS_CHROME_PX + TICK_LABEL_PADDING_PX
    expect(yAxisSize(660, ['1', wide, '2'])).toBe(expected)
    expect(yAxisSize(660, [wide, '1', '2'])).toBe(expected)
    expect(yAxisSize(660, ['1', '2', wide])).toBe(expected)
  })

  it('caps at a fraction of chart width when that fraction is still >= the floor', () => {
    const hugeLabel = 'a'.repeat(50)
    const chartWidth = 300
    const cap = Math.round(chartWidth * Y_AXIS_SIZE_FRACTION)
    expect(cap).toBeGreaterThan(Y_AXIS_BASE_SIZE_PX)
    expect(yAxisSize(chartWidth, [hugeLabel])).toBe(cap)
  })

  it('the fraction cap can never undercut the floor on a very narrow chart', () => {
    const hugeLabel = 'a'.repeat(50)
    expect(yAxisSize(10, [hugeLabel])).toBe(Y_AXIS_BASE_SIZE_PX)
  })

  it('caps at Y_AXIS_MAX_SIZE_PX for a huge label on a very wide chart', () => {
    const hugeLabel = 'a'.repeat(50)
    expect(yAxisSize(3000, [hugeLabel])).toBe(Y_AXIS_MAX_SIZE_PX)
  })
})

describe('buildYAxisConfig', () => {
  it('defaults show and showGrid to true and side to 3 (left)', () => {
    const axis = buildYAxisConfig({ conversionFactor: 1, displayUnit: '', currencyCode: null })
    expect(axis.show).toBe(true)
    expect(axis.side).toBe(3)
    expect(axis.grid).toEqual({ stroke: '#2a2a35', width: 1 })
  })

  it('passes scale/side/show through', () => {
    const axis = buildYAxisConfig({
      scale: 'redis_ops_per_sec',
      side: 1,
      show: false,
      conversionFactor: 1,
      displayUnit: '',
      currencyCode: null,
    })
    expect(axis.scale).toBe('redis_ops_per_sec')
    expect(axis.side).toBe(1)
    expect(axis.show).toBe(false)
  })

  it('showGrid: false yields a hidden grid', () => {
    const axis = buildYAxisConfig({ showGrid: false, conversionFactor: 1, displayUnit: '', currencyCode: null })
    expect(axis.grid).toEqual({ show: false })
  })

  it('values formatting matches formatYAxisTick for the plain case', () => {
    const axis = buildYAxisConfig({ conversionFactor: 1, displayUnit: 'ms', currencyCode: null })
    const fn = axis.values as (u: uPlot, vals: number[]) => string[]
    expect(fn(u, [123.456])).toEqual([formatYAxisTick(123.456, 1, 'ms', null)])
  })

  it('values formatting matches formatYAxisTick when the unit suffix is suppressed', () => {
    const axis = buildYAxisConfig({ conversionFactor: 1, displayUnit: '', currencyCode: null })
    const fn = axis.values as (u: uPlot, vals: number[]) => string[]
    expect(fn(u, [100])).toEqual([formatYAxisTick(100, 1, '', null)])
  })

  it('values formatting matches formatYAxisTick for a currency scale', () => {
    const axis = buildYAxisConfig({ conversionFactor: 1, displayUnit: 'USD', currencyCode: 'USD' })
    const fn = axis.values as (u: uPlot, vals: number[]) => string[]
    expect(fn(u, [1234.5])).toEqual([formatYAxisTick(1234.5, 1, 'USD', 'USD')])
  })

  it('size is a function returning the floor for null values (uPlot init call)', () => {
    const axis = buildYAxisConfig({ conversionFactor: 1, displayUnit: 'ms', currencyCode: null })
    const size = axis.size as (self: uPlot, values: string[] | null) => number
    const stubSelf = { width: 660 } as uPlot
    expect(size(stubSelf, null)).toBe(Y_AXIS_BASE_SIZE_PX)
  })

  it('size delegates to yAxisSize with the plot width and formatted values', () => {
    const axis = buildYAxisConfig({ conversionFactor: 1, displayUnit: 'ms', currencyCode: null })
    const size = axis.size as (self: uPlot, values: string[] | null) => number
    const stubSelf = { width: 660 } as uPlot
    const values = ['100 ops_per_sec']
    expect(size(stubSelf, values)).toBe(yAxisSize(660, values))
  })
})
