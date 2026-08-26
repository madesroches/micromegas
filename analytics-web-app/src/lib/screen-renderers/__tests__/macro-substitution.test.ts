import type { StructRowProxy } from 'apache-arrow'
import { Float64, List, Field } from 'apache-arrow'
import { formatArrowValue } from '../macro-substitution'
import { makeHistogramVector, SAMPLE_HISTOGRAM_ROW, HISTOGRAM_STRUCT_TYPE } from './histogram-fixtures'

describe('formatArrowValue — histogram struct branch', () => {
  it('renders a histogram-struct value as a compact, fixed-field-order dump', () => {
    const vec = makeHistogramVector([SAMPLE_HISTOGRAM_ROW])
    const raw = vec.get(0) as StructRowProxy

    const result = formatArrowValue(raw, HISTOGRAM_STRUCT_TYPE)

    expect(result).toBe(
      '{start:0, end:50, count:40, bins:[1,3,6,10,8,6,3,2,1,0]}'
    )
  })

  it('is a formatting improvement over the existing String(value) baseline, not a functional fix', () => {
    // Before this change, formatArrowValue's fallback was `String(value)`,
    // which already resolves to Arrow's own StructRow.toString() for a
    // struct value (readable, not "[object Object]"). The new branch
    // produces a different, more compact dump — confirm both are readable,
    // and that they differ (the new format wins on compactness/field order,
    // not on making an otherwise-broken case work).
    const vec = makeHistogramVector([SAMPLE_HISTOGRAM_ROW])
    const raw = vec.get(0) as StructRowProxy

    const baseline = String(raw)
    const withBranch = formatArrowValue(raw, HISTOGRAM_STRUCT_TYPE)

    expect(baseline).not.toBe('[object Object]')
    expect(withBranch).not.toBe(baseline)
    expect(withBranch).toContain('start:0')
    expect(withBranch).toContain('bins:[1,3,6,10,8,6,3,2,1,0]')
  })

  it('renders "null" for a null histogram value (falls through, no crash)', () => {
    const vec = makeHistogramVector([null])
    const raw = vec.get(0)
    expect(() => formatArrowValue(raw, HISTOGRAM_STRUCT_TYPE)).not.toThrow()
    expect(formatArrowValue(raw, HISTOGRAM_STRUCT_TYPE)).toBe(String(raw))
  })

  it('leaves a non-histogram struct/list/primitive value unaffected', () => {
    expect(formatArrowValue(42, new Float64())).toBe('42')
    expect(formatArrowValue('hello', undefined)).toBe('hello')
    expect(
      formatArrowValue([1, 2, 3], new List(new Field('item', new Float64())))
    ).toBe(String([1, 2, 3]))
  })

  it('leaves an unrelated struct-shaped value unaffected (not histogram-shaped)', () => {
    // A struct with a different field set is not histogram-typed, so the
    // fallback String(value) still applies — asserted indirectly via
    // isHistogramStructType's own coverage in arrow-utils.test.ts; here we
    // just confirm formatArrowValue doesn't blow up without a dataType.
    expect(formatArrowValue({ a: 1, b: 2 }, undefined)).toBe(String({ a: 1, b: 2 }))
  })
})
