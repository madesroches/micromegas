import {
  Table,
  Timestamp,
  TimeUnit,
  tableFromArrays,
  vectorFromArray,
} from 'apache-arrow'
import { mapMetadata } from '../MapCell'
import { buildOverlay, materializeRow } from '@/components/map/overlay'
import { DEFAULT_MAP_DETAIL_TEMPLATE } from '../../notebook-utils'

describe('buildOverlay', () => {
  it('returns ok with a row-ordered positions buffer of length numRows * 3', () => {
    const table = tableFromArrays({
      x: new Float64Array([1.5, 4.5]),
      y: new Float64Array([2.5, 5.5]),
      z: new Float64Array([3.5, 6.5]),
    })
    const result = buildOverlay(table)
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.overlay.positions).toHaveLength(6)
    expect(Array.from(result.overlay.positions)).toEqual([1.5, 2.5, 3.5, 4.5, 5.5, 6.5])
    expect(result.overlay.table).toBe(table)
  })

  it('returns ok: false naming the offending row when x is non-finite', () => {
    const table = tableFromArrays({
      x: new Float64Array([1, NaN]),
      y: new Float64Array([1, 1]),
      z: new Float64Array([1, 1]),
    })
    const result = buildOverlay(table)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/Row 1/)
    expect(result.error).toMatch(/non-finite/)
  })

  it('returns ok: false when required columns are missing', () => {
    const table = tableFromArrays({
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      // z missing
    })
    const result = buildOverlay(table)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/Missing required columns/)
    expect(result.error).toMatch(/z/)
  })

  it('returns ok: false when x/y/z exist but are not numeric', () => {
    const table = tableFromArrays({
      x: ['a', 'b'],
      y: new Float64Array([0, 0]),
      z: new Float64Array([0, 0]),
    })
    const result = buildOverlay(table)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/'x'/)
    expect(result.error).toMatch(/numeric/)
  })
})

describe('materializeRow', () => {
  it('formats every non-null column as a string', () => {
    const table = tableFromArrays({
      process_id: ['p1'],
      x: new Float64Array([1.5]),
      y: new Float64Array([2.5]),
      z: new Float64Array([3.5]),
      event_type: ['hit'],
    })
    expect(materializeRow(table, 0)).toEqual({
      process_id: 'p1',
      x: '1.5',
      y: '2.5',
      z: '3.5',
      event_type: 'hit',
    })
  })

  it('omits columns whose value is null (no empty-string coercion)', () => {
    const table = tableFromArrays({
      process_id: ['p1'],
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      z: new Float64Array([0]),
      maybe_null: [null as string | null],
    })
    const row = materializeRow(table, 0)
    expect(row).not.toHaveProperty('maybe_null')
    expect(row.process_id).toBe('p1')
  })

  it('formats timestamp columns as RFC3339', () => {
    const timestampType = new Timestamp(TimeUnit.MILLISECOND, null)
    const timeVec = vectorFromArray([1705314600000], timestampType)
    const xVec = vectorFromArray([0])
    const yVec = vectorFromArray([0])
    const zVec = vectorFromArray([0])
    const table = new Table({ time: timeVec, x: xVec, y: yVec, z: zVec })
    expect(materializeRow(table, 0).time).toBe('2024-01-15T10:30:00.000Z')
  })
})

describe('mapMetadata', () => {
  it('seeds detailTemplate in createDefaultConfig', () => {
    const config = mapMetadata.createDefaultConfig() as { options?: Record<string, unknown> }
    expect(config.options?.detailTemplate).toBe(DEFAULT_MAP_DETAIL_TEMPLATE)
  })

  it('declares single-row selection mode by default', () => {
    expect(mapMetadata.defaultSelectionMode).toBe('single')
  })
})
