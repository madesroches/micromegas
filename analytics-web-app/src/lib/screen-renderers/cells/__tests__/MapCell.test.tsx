import {
  Table,
  Timestamp,
  TimeUnit,
  tableFromArrays,
  vectorFromArray,
} from 'apache-arrow'
import { arrowTableToMapEvents, mapMetadata } from '../MapCell'
import { DEFAULT_MAP_DETAIL_TEMPLATE } from '../../notebook-utils'

describe('arrowTableToMapEvents', () => {
  it('stores every non-null column on row as a string', () => {
    const table = tableFromArrays({
      process_id: ['p1'],
      x: new Float64Array([1.5]),
      y: new Float64Array([2.5]),
      z: new Float64Array([3.5]),
      event_type: ['hit'],
    })
    const events = arrowTableToMapEvents(table)
    expect(events).toHaveLength(1)
    const [event] = events
    expect(event.id).toBe('p1-0')
    expect(event.x).toBeCloseTo(1.5)
    expect(event.y).toBeCloseTo(2.5)
    expect(event.z).toBeCloseTo(3.5)
    expect(event.row).toEqual({
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
    const events = arrowTableToMapEvents(table)
    expect(events).toHaveLength(1)
    expect(events[0].row).not.toHaveProperty('maybe_null')
  })

  it('formats timestamp columns as RFC3339 when present', () => {
    const timestampType = new Timestamp(TimeUnit.MILLISECOND, null)
    const timeVec = vectorFromArray([1705314600000], timestampType)
    const xVec = vectorFromArray([0])
    const yVec = vectorFromArray([0])
    const zVec = vectorFromArray([0])
    const table = new Table({ time: timeVec, x: xVec, y: yVec, z: zVec })

    const events = arrowTableToMapEvents(table)
    expect(events).toHaveLength(1)
    expect(events[0].time).toBeInstanceOf(Date)
    expect(events[0].row.time).toBe('2024-01-15T10:30:00.000Z')
  })

  it('leaves time undefined when no time column is present', () => {
    const table = tableFromArrays({
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      z: new Float64Array([0]),
    })
    const events = arrowTableToMapEvents(table)
    expect(events).toHaveLength(1)
    expect(events[0].time).toBeUndefined()
    expect(events[0].row).not.toHaveProperty('time')
  })

  it('skips rows whose x/y/z are non-numeric (NaN), defaults missing to 0', () => {
    const table = tableFromArrays({
      x: new Float64Array([1, NaN]),
      y: new Float64Array([1, 1]),
      z: new Float64Array([1, 1]),
    })
    const events = arrowTableToMapEvents(table)
    expect(events).toHaveLength(1)
    expect(events[0].x).toBe(1)
  })

  it('derives id from process_id when present, "unknown" otherwise', () => {
    const tableWithPid = tableFromArrays({
      process_id: ['abc'],
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      z: new Float64Array([0]),
    })
    const tableNoPid = tableFromArrays({
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      z: new Float64Array([0]),
    })
    expect(arrowTableToMapEvents(tableWithPid)[0].id).toBe('abc-0')
    expect(arrowTableToMapEvents(tableNoPid)[0].id).toBe('unknown-0')
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
