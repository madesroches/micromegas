import type { StructRowProxy } from 'apache-arrow'
import { toHistogramValue, estimateHistogramQuantile, bucketRange } from '../histogram-utils'
import { makeHistogramVector, SAMPLE_HISTOGRAM_ROW } from '../screen-renderers/__tests__/histogram-fixtures'

describe('toHistogramValue', () => {
  it('decodes count (bigint) and bins (Vector<bigint>) to plain numbers', () => {
    const vec = makeHistogramVector([SAMPLE_HISTOGRAM_ROW])
    const raw = vec.get(0) as StructRowProxy

    // Sanity-check the raw shape actually has bigint/Vector fields (i.e. the
    // fixture is exercising the real hazard, not silently falling back to
    // Float64).
    expect(typeof raw.count).toBe('bigint')

    const h = toHistogramValue(raw)
    expect(typeof h.count).toBe('number')
    expect(h.count).toBe(40)
    expect(Array.isArray(h.bins)).toBe(true)
    expect(h.bins.every((v) => typeof v === 'number')).toBe(true)
    expect(h.bins).toEqual([1, 3, 6, 10, 8, 6, 3, 2, 1, 0])
    expect(h.start).toBe(0)
    expect(h.end).toBe(50)
    expect(h.min).toBe(1)
    expect(h.max).toBe(48)
    expect(h.sum).toBe(1200)
    expect(h.sum_sq).toBe(60000)
  })

  it('decodes a null struct row via a null-checked caller (no null branch of its own)', () => {
    const vec = makeHistogramVector([null])
    const raw = vec.get(0)
    expect(raw).toBeNull()
  })
})

describe('estimateHistogramQuantile', () => {
  it('estimates the median via linear interpolation within the crossing bucket', () => {
    // 4 bins over [0, 40): [10, 10, 10, 10], count = 40. Median target = 20,
    // reached exactly at the end boundary of bucket index 1 (cumulative
    // count crosses 20 there, bucket_ratio = 1.0 -> estimate = end_bucket).
    const h = { start: 0, end: 40, min: 0, max: 40, sum: 0, sum_sq: 0, count: 40, bins: [10, 10, 10, 10] }
    expect(estimateHistogramQuantile(h, 0.5)).toBeCloseTo(20, 5)
  })

  it('interpolates within a bucket for a ratio that lands mid-bucket', () => {
    // bins: [0, 40] count=40 -> target 0.25 => quant_count=10, reached
    // exactly at the end of bucket 0 (cumulative 10). Use ratio 0.375 to land
    // mid-bucket-1 instead: quant_count = 15, bucket 1 spans cumulative
    // (10, 20], ratio into bucket = (15-10)/10 = 0.5 -> x = 10 + 0.5*10 = 15
    const h = { start: 0, end: 40, min: 0, max: 40, sum: 0, sum_sq: 0, count: 40, bins: [10, 10, 10, 10] }
    expect(estimateHistogramQuantile(h, 0.375)).toBeCloseTo(15, 5)
  })

  it('returns `end` when count is 0 (no bucket crosses the target ratio)', () => {
    const h = { start: 0, end: 40, min: 0, max: 0, sum: 0, sum_sq: 0, count: 0, bins: [0, 0, 0, 0] }
    expect(estimateHistogramQuantile(h, 0.5)).toBe(40)
  })

  it('returns start unchanged for a degenerate point histogram (start === end)', () => {
    const h = { start: 5, end: 5, min: 5, max: 5, sum: 50, sum_sq: 500, count: 10, bins: [10] }
    expect(estimateHistogramQuantile(h, 0.5)).toBe(5)
  })
})

describe('bucketRange', () => {
  it('returns the [start, end) boundaries for a bucket index', () => {
    const h = { start: 0, end: 40, min: 0, max: 40, sum: 0, sum_sq: 0, count: 40, bins: [10, 10, 10, 10] }
    expect(bucketRange(h, 0)).toEqual([0, 10])
    expect(bucketRange(h, 2)).toEqual([20, 30])
    expect(bucketRange(h, 3)).toEqual([30, 40])
  })

  it('falls back to unit bin width for a degenerate point histogram', () => {
    const h = { start: 5, end: 5, min: 5, max: 5, sum: 50, sum_sq: 500, count: 10, bins: [10] }
    expect(bucketRange(h, 0)).toEqual([5, 6])
  })
})
