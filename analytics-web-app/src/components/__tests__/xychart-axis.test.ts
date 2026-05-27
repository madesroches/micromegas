import type uPlot from 'uplot'
import { buildXAxisConfig } from '../xychart-axis'

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
