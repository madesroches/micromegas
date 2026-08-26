/**
 * Shared test fixture for building `Histogram`-typed struct columns with the
 * exact `UInt64`/`List<UInt64>` field types the Rust side produces.
 *
 * A naive `vectorFromArray([{count: 5, bins: [1, 2, 3]}])` infers
 * `Float64`/`List<Float64>` from plain JS numbers — that still satisfies
 * `isHistogramStructType` (it only checks field names/order and that the
 * last field is a `List`), but never produces a single `bigint`, silently
 * skipping the bigint/`Vector` read-boundary hazard `toHistogramValue`
 * exists to normalize (see `tasks/histogram_column_cell_plan.md`, Testing
 * Strategy).
 */

import { Field, Float64, List, Struct, Table, Uint64, Utf8, vectorFromArray } from 'apache-arrow'

/** The `Histogram` struct type, field-for-field identical to
 *  `make_histogram()`'s output (`accumulator.rs`'s `state_arrow_fields()`). */
export const HISTOGRAM_STRUCT_TYPE = new Struct([
  new Field('start', new Float64()),
  new Field('end', new Float64()),
  new Field('min', new Float64()),
  new Field('max', new Float64()),
  new Field('sum', new Float64()),
  new Field('sum_sq', new Float64()),
  new Field('count', new Uint64()),
  new Field('bins', new List(new Field('item', new Uint64()))),
])

export interface HistogramRowInput {
  start: number
  end: number
  min: number
  max: number
  sum: number
  sum_sq: number
  count: number | bigint
  bins: (number | bigint)[]
}

/**
 * Builds a `Vector<Struct>` for a histogram-typed column. `null` entries
 * produce a null struct row (mirrors the unconfigured-accumulator path,
 * where Arrow JS delivers `null` rather than a non-null struct with empty
 * `bins`).
 */
export function makeHistogramVector(rows: (HistogramRowInput | null)[]) {
  return vectorFromArray(
    rows.map((r) =>
      r === null
        ? null
        : {
            start: r.start,
            end: r.end,
            min: r.min,
            max: r.max,
            sum: r.sum,
            sum_sq: r.sum_sq,
            count: BigInt(r.count),
            bins: r.bins.map((b) => BigInt(b)),
          }
    ),
    HISTOGRAM_STRUCT_TYPE
  )
}

/**
 * Builds a two-column Table (`name`: Utf8, `dist`: Histogram struct) — the
 * shape most tests need: a label column plus one histogram column.
 */
export function makeHistogramTable(rows: { name: string; dist: HistogramRowInput | null }[]): Table {
  const nameVec = vectorFromArray(
    rows.map((r) => r.name),
    new Utf8()
  )
  const distVec = makeHistogramVector(rows.map((r) => r.dist))
  return new Table({ name: nameVec, dist: distVec })
}

/** A representative, non-degenerate histogram row for tests that don't care
 *  about the specific shape. */
export const SAMPLE_HISTOGRAM_ROW: HistogramRowInput = {
  start: 0,
  end: 50,
  min: 1,
  max: 48,
  sum: 1200,
  sum_sq: 60000,
  count: 40,
  bins: [1, 3, 6, 10, 8, 6, 3, 2, 1, 0],
}
