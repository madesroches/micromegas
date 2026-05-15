import {
  Binary,
  Dictionary,
  Int32,
  Table,
  Timestamp,
  TimeUnit,
  Utf8,
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

  it('default mapping leaves colorsRGBA undefined and puts #bf360cff in constants.color', () => {
    // Scalar color is not materialized into a per-row buffer — the renderer
    // reads `constants.color` instead. This is what keeps editor-side color
    // scrubbing from triggering a full O(numRows) overlay rebuild.
    const table = tableFromArrays({
      x: new Float64Array([0, 0]),
      y: new Float64Array([0, 0]),
      z: new Float64Array([0, 0]),
    })
    const result = buildOverlay(table)
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.overlay.colorsRGBA).toBeUndefined()
    expect(result.constants.color).toBe(0xbf360cff)
    // Default mapping is sphere; no scales/sizes buffers when channels are scalar.
    expect(result.overlay.sizes).toBeUndefined()
    expect(result.overlay.scales).toBeUndefined()
    expect(result.constants.size).toBe(10)
  })

  it('writes per-instance scales only for column-bound channels (box mixed)', () => {
    // Mixed mapping: scaleX is column-bound, scaleY/scaleZ are scalar. The
    // baked buffer must NOT pin the scalar values into its slots — the
    // renderer reads `constants.scale[k]` for scalar channels via
    // `scaleColumnMask`, so editor edits to scaleY/scaleZ aren't lost.
    const table = tableFromArrays({
      x: new Float64Array([0, 0]),
      y: new Float64Array([0, 0]),
      z: new Float64Array([0, 0]),
      sx: new Float64Array([7, 11]),
    })
    const result = buildOverlay(table, {
      scaleX: { column: 'sx' },
      scaleY: { scalar: 100 },
      scaleZ: { scalar: 100 },
    })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.overlay.scales).toBeDefined()
    // Only the X slots are written; Y/Z slots stay zero-initialized.
    expect(Array.from(result.overlay.scales!)).toEqual([7, 0, 0, 11, 0, 0])
    expect(result.overlay.scaleColumnMask).toEqual([true, false, false])
    // The scalar fallbacks live in constants for the renderer to pick up
    // on every render, untouched by buildOverlay's row walk.
    expect(result.constants.scale).toEqual([100, 100, 100])
  })

  it('reads Int32 color column as u32, including high-bit-set values', () => {
    // 0xbf360cff comes back from Arrow Int32 as a negative JS number
    // (-1086357249). The signed→unsigned coercion in writeRGBA must preserve
    // the bit pattern.
    const colorVec = vectorFromArray(new Int32Array([0xbf360cff | 0]))
    const xVec = vectorFromArray(new Float64Array([0]))
    const yVec = vectorFromArray(new Float64Array([0]))
    const zVec = vectorFromArray(new Float64Array([0]))
    const table = new Table({ x: xVec, y: yVec, z: zVec, c: colorVec })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([0xbf, 0x36, 0x0c, 0xff])
  })

  it('reads Int64 (bigint) color column as u32', () => {
    // DataFusion infers integer literals like 0xbf360cff as Int64 by default;
    // Arrow JS returns bigint from col.get(i). The coercion path must avoid
    // the TypeError that >>> would throw on a bigint.
    const colorVec = vectorFromArray(new BigInt64Array([0xbf360cffn]))
    const xVec = vectorFromArray(new Float64Array([0]))
    const yVec = vectorFromArray(new Float64Array([0]))
    const zVec = vectorFromArray(new Float64Array([0]))
    const table = new Table({ x: xVec, y: yVec, z: zVec, c: colorVec })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([0xbf, 0x36, 0x0c, 0xff])
  })

  it('parses string color column with #rrggbb (alpha defaults to 0xff)', () => {
    const table = tableFromArrays({
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      z: new Float64Array([0]),
      c: ['#00ff80'],
    })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([0x00, 0xff, 0x80, 0xff])
  })

  it('parses string color column with #rrggbbaa', () => {
    const table = tableFromArrays({
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      z: new Float64Array([0]),
      c: ['#11223344'],
    })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([0x11, 0x22, 0x33, 0x44])
  })

  it('parses dictionary-encoded Utf8 color column (the CASE WHEN case)', () => {
    // A literal '#rrggbbaa' in a CASE WHEN arrives as Dictionary<Int32, Utf8>
    // in Arrow IPC. A naked isStringType check would reject it; the unwrap
    // path must accept it.
    const dictType = new Dictionary(new Utf8(), new Int32())
    const colorVec = vectorFromArray(['#11223344', '#11223344'], dictType)
    const xVec = vectorFromArray(new Float64Array([0, 0]))
    const yVec = vectorFromArray(new Float64Array([0, 0]))
    const zVec = vectorFromArray(new Float64Array([0, 0]))
    const table = new Table({ x: xVec, y: yVec, z: zVec, c: colorVec })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([
      0x11, 0x22, 0x33, 0x44,
      0x11, 0x22, 0x33, 0x44,
    ])
  })

  it('returns ok: false naming the row for an unparseable color string', () => {
    const table = tableFromArrays({
      x: new Float64Array([0, 0]),
      y: new Float64Array([0, 0]),
      z: new Float64Array([0, 0]),
      c: ['#11223344', 'red'],
    })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/Row 1/)
    expect(result.error).toMatch(/unparseable/)
  })

  it('returns ok: false when color column is neither integer, string, nor binary', () => {
    const table = tableFromArrays({
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      z: new Float64Array([0]),
      c: new Float64Array([1.5]),
    })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/'c'/)
    expect(result.error).toMatch(/must be integer/)
    expect(result.error).toMatch(/binary/)
  })

  it('reads Binary color column as packed R,G,B,A (the 0xrrggbbaa SQL literal case)', () => {
    // DataFusion parses `0xff0000ff` as a 4-byte Binary literal, not an int.
    // The bytes come back from Arrow JS as a Uint8Array in big-endian order,
    // which we copy straight into the RGBA buffer.
    const colorVec = vectorFromArray(
      [new Uint8Array([0xff, 0x00, 0x00, 0xff])],
      new Binary(),
    )
    const xVec = vectorFromArray(new Float64Array([0]))
    const yVec = vectorFromArray(new Float64Array([0]))
    const zVec = vectorFromArray(new Float64Array([0]))
    const table = new Table({ x: xVec, y: yVec, z: zVec, c: colorVec })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([0xff, 0x00, 0x00, 0xff])
  })

  it('returns ok: false naming the row when a Binary color cell is not exactly 4 bytes', () => {
    const colorVec = vectorFromArray(
      [
        new Uint8Array([0xff, 0x00, 0x00, 0xff]),
        new Uint8Array([0xff, 0x00]),
      ],
      new Binary(),
    )
    const xVec = vectorFromArray(new Float64Array([0, 0]))
    const yVec = vectorFromArray(new Float64Array([0, 0]))
    const zVec = vectorFromArray(new Float64Array([0, 0]))
    const table = new Table({ x: xVec, y: yVec, z: zVec, c: colorVec })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/Row 1/)
    expect(result.error).toMatch(/2 bytes/)
  })

  it('returns ok: false naming the row for a non-finite numeric channel', () => {
    const table = tableFromArrays({
      x: new Float64Array([0, 0]),
      y: new Float64Array([0, 0]),
      z: new Float64Array([0, 0]),
      sx: new Float64Array([1, NaN]),
    })
    const result = buildOverlay(table, {
      scaleX: { column: 'sx' },
      scaleY: { scalar: 1 },
      scaleZ: { scalar: 1 },
    })
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/Row 1/)
    expect(result.error).toMatch(/non-finite/)
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

  it('seeds shape=sphere with size/color scalars in createDefaultConfig', () => {
    const config = mapMetadata.createDefaultConfig() as {
      options?: { shape?: string; mapping?: Record<string, unknown> }
    }
    expect(config.options?.shape).toBe('sphere')
    expect(config.options?.mapping).toEqual({
      size: { scalar: 10 },
      color: { scalar: 0xbf360cff },
    })
  })

  it('declares single-row selection mode by default', () => {
    expect(mapMetadata.defaultSelectionMode).toBe('single')
  })
})
