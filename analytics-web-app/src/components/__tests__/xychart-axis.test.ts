import type uPlot from 'uplot'
import { buildXAxisConfig, buildXScale, formatYAxisTick } from '../xychart-axis'

// The `values` formatter ignores its uPlot argument; pass a stub.
const u = undefined as unknown as uPlot

describe('buildXAxisConfig', () => {
  it('time mode leaves values/incrs unset (uPlot uses its time defaults)', () => {
    const axis = buildXAxisConfig('time')
    expect(axis.values).toBeUndefined()
    expect(axis.incrs).toBeUndefined()
    expect(axis.size).toBe(65)
  })

  it('categorical mode maps tick indices to labels and blanks out-of-range', () => {
    const axis = buildXAxisConfig('categorical', ['a', 'b', 'c'])
    expect(axis.incrs).toEqual([1])
    const fn = axis.values as (u: uPlot, vals: number[]) => string[]
    expect(fn(u, [0, 1, 2, 3])).toEqual(['a', 'b', 'c', ''])
    // Rounds fractional tick positions to the nearest index.
    expect(fn(u, [1.4])).toEqual(['b'])
  })

  it('categorical without labels falls through to the default (no values)', () => {
    const axis = buildXAxisConfig('categorical')
    expect(axis.values).toBeUndefined()
  })

  it('numeric mode abbreviates with magnitude-dependent precision', () => {
    const axis = buildXAxisConfig('numeric')
    const fn = axis.values as (u: uPlot, vals: number[]) => string[]
    expect(fn(u, [0])).toEqual(['0'])
    expect(fn(u, [12345])).toEqual([(12345).toLocaleString()])
    expect(fn(u, [3.14159])).toEqual(['3.1'])
    expect(fn(u, [0.0123])).toEqual([(0.0123).toPrecision(2)])
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
})
