/**
 * Pure functions for working with `Histogram`-typed struct values (the
 * Arrow struct produced by the `make_histogram()` SQL aggregate). Shared by
 * the default per-row histogram cell rendering and the debug-format path
 * (`macro-substitution.ts`'s `formatArrowValue`).
 *
 * See `tasks/histogram_column_cell_plan.md`, Design §4, Implementation
 * Steps §2.
 */

import type { StructRowProxy } from 'apache-arrow'

/**
 * Normalized histogram value: plain `number`s only. `count` (`UInt64` ->
 * `bigint` on the raw Arrow struct) and each element of `bins`
 * (`List<UInt64>` -> `Vector<bigint>`, no numeric index signature) are
 * converted once here, at the read boundary — nothing downstream touches a
 * `bigint` or a `Vector`.
 */
export interface HistogramValue {
  start: number
  end: number
  min: number
  max: number
  sum: number
  sum_sq: number
  count: number
  bins: number[]
}

/**
 * Reads a raw `Histogram` struct cell (a `StructRowProxy`) into a
 * `HistogramValue`. `start`/`end`/`min`/`max`/`sum`/`sum_sq` are `Float64`
 * fields that already decode to plain `number`; `count` is `UInt64`
 * (`bigint`) and `bins` is a `List<UInt64>` (`Vector<bigint>`, read via
 * `Array.from` since it has no numeric index signature).
 */
export function toHistogramValue(raw: StructRowProxy): HistogramValue {
  return {
    start: Number(raw.start),
    end: Number(raw.end),
    min: Number(raw.min),
    max: Number(raw.max),
    sum: Number(raw.sum),
    sum_sq: Number(raw.sum_sq),
    count: Number(raw.count),
    bins: Array.from(raw.bins as Iterable<bigint>, Number),
  }
}

/**
 * Estimates the value at `ratio` (e.g. `0.5` for the median) via linear
 * interpolation within the bucket whose cumulative count first reaches
 * `count * ratio`. A straight port of `estimate_quantile`
 * (`rust/datafusion-extensions/src/histogram/quantile.rs`), including its
 * `return end` fallback when no bucket's cumulative count reaches the
 * target (e.g. `count === 0`).
 *
 * No `start === end` epsilon guard here: with zero width the loop below
 * still returns `start` unchanged (every bucket boundary collapses to
 * `start`), which is the correct median for a point histogram. The
 * `0/0 -> NaN` hazard only shows up in the tick's *position* formula
 * (`HistogramCell`'s `((median - start) / (end - start)) * 120`), not in
 * this value, and is guarded there instead.
 */
export function estimateHistogramQuantile(h: HistogramValue, ratio: number): number {
  const quantCount = h.count * ratio
  const bucketWidth = (h.end - h.start) / h.bins.length
  let cumulative = 0
  for (let i = 0; i < h.bins.length; i++) {
    const bucketCount = h.bins[i]
    cumulative += bucketCount
    if (cumulative >= quantCount && bucketCount > 0) {
      const popBucketStart = cumulative - bucketCount
      const popBucketEnd = cumulative
      const bucketRatio = (quantCount - popBucketStart) / (popBucketEnd - popBucketStart)
      const beginBucket = h.start + i * bucketWidth
      const endBucket = h.start + (i + 1) * bucketWidth
      return (1 - bucketRatio) * beginBucket + bucketRatio * endBucket
    }
  }
  return h.end
}

/**
 * Returns bucket `bucketIndex`'s `[start, end)` boundaries. Port of
 * `expand.rs`'s bin-width math, including its
 * `Math.abs(end - start) < Number.EPSILON -> bin_width = 1.0` fallback for
 * the degenerate point-histogram case (`start === end`).
 */
export function bucketRange(h: HistogramValue, bucketIndex: number): [number, number] {
  const binWidth =
    Math.abs(h.end - h.start) < Number.EPSILON ? 1.0 : (h.end - h.start) / h.bins.length
  const bucketStart = h.start + bucketIndex * binWidth
  const bucketEnd = h.start + (bucketIndex + 1) * binWidth
  return [bucketStart, bucketEnd]
}
