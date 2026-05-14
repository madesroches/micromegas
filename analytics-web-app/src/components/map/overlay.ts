import type { Table } from 'apache-arrow'
import {
  isIntegerType,
  isNumericType,
  isStringType,
  unwrapDictionary,
} from '@/lib/arrow-utils'
import { formatArrowValue } from '@/lib/screen-renderers/notebook-utils'

/** A single row of the SQL result, formatted to strings for template substitution. */
export type Row = Record<string, string>

export type Shape = 'sphere' | 'box'

/**
 * One channel of an overlay mapping: either a literal value to apply to every
 * row (`scalar`), or the name of a column to read per-row values from (`column`).
 */
export type ChannelBinding<T = number> =
  | { column: string }
  | { scalar: T }

/**
 * Per-channel column-or-scalar bindings. Channel semantics depend on `shape`:
 * - sphere uses `size` (uniform radius multiplier in world units)
 * - box uses `scaleX/scaleY/scaleZ` (non-uniform world-unit extents)
 * - both use `color` (RGBA u32 scalar, or column of integer/string RGBA)
 *
 * `x/y/z` default to the reserved `x`/`y`/`z` column names; overrides let SQL
 * authors emit position columns under different names.
 */
export interface OverlayMapping {
  x?: ChannelBinding<string>
  y?: ChannelBinding<string>
  z?: ChannelBinding<string>

  size?: ChannelBinding
  scaleX?: ChannelBinding
  scaleY?: ChannelBinding
  scaleZ?: ChannelBinding

  /**
   * Color binding. Scalar payload is an RGBA u32 (0xrrggbbaa). A column
   * binding may reference either an integer column (read bit-for-bit as
   * u32) or a string column (parsed as '#rrggbb' or '#rrggbbaa').
   */
  color?: ChannelBinding
}

export interface Overlay {
  table: Table
  /** Flat [x0,y0,z0, x1,y1,z1, ...] in row order. Length = numRows * 3. */
  positions: Float32Array
  /** Per-instance RGBA bytes — [r,g,b,a, r,g,b,a, ...]. Length = numRows * 4.
   *  Always materialized: at 4 bytes/row the buffer is 0.4 MB even at 100K
   *  rows, small enough that the "constant fill vs per-row" optimization is
   *  not worth a second code path in the renderer. */
  colorsRGBA: Uint8Array
  /** Non-uniform per-instance scale [sx0,sy0,sz0, ...]. Length = numRows * 3.
   *  Allocated iff any of scaleX/scaleY/scaleZ is column-bound (box only).
   *  When absent, the renderer reads `constants.scale` for every instance. */
  scales?: Float32Array
  /** Uniform per-instance scale [s0,s1,...]. Length = numRows. Allocated iff
   *  `size` is column-bound (sphere only). When absent, the renderer reads
   *  `constants.size` for every instance. */
  sizes?: Float32Array
}

/** Scalar fallbacks for size channels. Color is always in `Overlay.colorsRGBA`. */
export interface OverlayConstants {
  /** Sphere fallback when `overlay.sizes` is absent. */
  size: number
  /** Box fallback when `overlay.scales` is absent. [sx, sy, sz]. */
  scale: [number, number, number]
}

export type OverlayResult =
  | { ok: true; overlay: Overlay; constants: OverlayConstants }
  | { ok: false; error: string }

const DEFAULT_RGBA = 0xbf360cff
const DEFAULT_SPHERE_SIZE = 10
const DEFAULT_BOX_SCALE: [number, number, number] = [100, 100, 100]

/** Parses '#rrggbb' (alpha=ff) or '#rrggbbaa' to a u32. Returns null on
 *  malformed input — callers decide whether that is fatal. */
export function rgbaFromHex(s: string): number | null {
  if (typeof s !== 'string') return null
  if (s.length !== 7 && s.length !== 9) return null
  if (s.charCodeAt(0) !== 35 /* '#' */) return null
  const hex = s.slice(1)
  for (let i = 0; i < hex.length; i++) {
    const c = hex.charCodeAt(i)
    const isDigit = c >= 48 && c <= 57
    const isLowerHex = c >= 97 && c <= 102
    const isUpperHex = c >= 65 && c <= 70
    if (!isDigit && !isLowerHex && !isUpperHex) return null
  }
  const r = parseInt(hex.slice(0, 2), 16)
  const g = parseInt(hex.slice(2, 4), 16)
  const b = parseInt(hex.slice(4, 6), 16)
  const a = hex.length === 8 ? parseInt(hex.slice(6, 8), 16) : 0xff
  // `>>> 0` reinterprets the signed bit-or result as unsigned u32.
  return (((r << 24) | (g << 16) | (b << 8) | a) >>> 0)
}

/** Format an RGBA u32 as `#rrggbbaa`. */
export function hexFromRgba(rgba: number): string {
  const u = rgba >>> 0
  const hex = u.toString(16).padStart(8, '0')
  return `#${hex}`
}

/** Writes a 4-byte RGBA value into a per-instance buffer at row `i`. */
export function writeRGBA(buf: Uint8Array, i: number, rgba: number): void {
  const base = i * 4
  buf[base] = (rgba >>> 24) & 0xff
  buf[base + 1] = (rgba >>> 16) & 0xff
  buf[base + 2] = (rgba >>> 8) & 0xff
  buf[base + 3] = rgba & 0xff
}

/** Default mapping for `shape`. No access to cell options — back-compat
 *  with legacy markerSize/markerColor is the cell's job, not the builder's. */
export function defaultMappingFor(shape: Shape): OverlayMapping {
  if (shape === 'sphere') {
    return {
      size: { scalar: DEFAULT_SPHERE_SIZE },
      color: { scalar: DEFAULT_RGBA },
    }
  }
  return {
    scaleX: { scalar: DEFAULT_BOX_SCALE[0] },
    scaleY: { scalar: DEFAULT_BOX_SCALE[1] },
    scaleZ: { scalar: DEFAULT_BOX_SCALE[2] },
    color: { scalar: DEFAULT_RGBA },
  }
}

function resolvePositionColumn(
  binding: ChannelBinding<string> | undefined,
  fallback: string,
): string {
  if (!binding) return fallback
  if ('column' in binding) return binding.column
  return binding.scalar
}

function isBoxMapping(mapping: OverlayMapping): boolean {
  return (
    mapping.scaleX !== undefined ||
    mapping.scaleY !== undefined ||
    mapping.scaleZ !== undefined
  )
}

/**
 * Coerce an Arrow column cell into a u32. Int64/UInt64 columns return
 * `bigint` from `col.get(i)`; smaller integer columns return `number`
 * (which may be signed-negative for high-bit-set UInt32 — `>>> 0`
 * reinterprets as unsigned without changing the bit pattern).
 */
function coerceCellToU32(value: unknown): number {
  if (typeof value === 'bigint') {
    return Number(value & 0xffffffffn)
  }
  return (value as number) >>> 0
}

export function buildOverlay(
  table: Table,
  mapping?: OverlayMapping,
): OverlayResult {
  const m = mapping ?? defaultMappingFor('sphere')

  const xName = resolvePositionColumn(m.x, 'x')
  const yName = resolvePositionColumn(m.y, 'y')
  const zName = resolvePositionColumn(m.z, 'z')

  const missing: string[] = []
  for (const name of [xName, yName, zName]) {
    if (!table.getChild(name)) missing.push(name)
  }
  if (missing.length > 0) {
    const available = table.schema.fields.map((f) => f.name).join(', ')
    return {
      ok: false,
      error: `Missing required columns: ${missing.join(', ')}. Available: ${available}`,
    }
  }

  for (const name of [xName, yName, zName]) {
    const field = table.schema.fields.find((f) => f.name === name)!
    if (!isNumericType(unwrapDictionary(field.type))) {
      return {
        ok: false,
        error: `Column '${name}' must be numeric, got ${field.type.toString()}`,
      }
    }
  }

  // Validate every numeric channel that's column-bound.
  const numericChannels: { label: string; binding: ChannelBinding | undefined }[] = [
    { label: 'size', binding: m.size },
    { label: 'scaleX', binding: m.scaleX },
    { label: 'scaleY', binding: m.scaleY },
    { label: 'scaleZ', binding: m.scaleZ },
  ]
  for (const { label, binding } of numericChannels) {
    if (!binding || !('column' in binding)) continue
    const field = table.schema.fields.find((f) => f.name === binding.column)
    if (!field) {
      return {
        ok: false,
        error: `Column '${binding.column}' for channel '${label}' not found in result`,
      }
    }
    if (!isNumericType(unwrapDictionary(field.type))) {
      return {
        ok: false,
        error: `Column '${binding.column}' for channel '${label}' must be numeric, got ${field.type.toString()}`,
      }
    }
  }

  // Validate color column (integer or string).
  let colorColumnKind: 'integer' | 'string' | null = null
  let colorColumnName: string | null = null
  if (m.color && 'column' in m.color) {
    colorColumnName = m.color.column
    const field = table.schema.fields.find((f) => f.name === colorColumnName)
    if (!field) {
      return {
        ok: false,
        error: `Column '${colorColumnName}' for channel 'color' not found in result`,
      }
    }
    const innerType = unwrapDictionary(field.type)
    if (isIntegerType(innerType)) {
      colorColumnKind = 'integer'
    } else if (isStringType(innerType)) {
      colorColumnKind = 'string'
    } else {
      return {
        ok: false,
        error: `Column '${colorColumnName}' for channel 'color' must be integer or string, got ${field.type.toString()}`,
      }
    }
  }

  const numRows = table.numRows
  const xCol = table.getChild(xName)!
  const yCol = table.getChild(yName)!
  const zCol = table.getChild(zName)!

  const positions = new Float32Array(numRows * 3)

  // Size buffers — allocated only when the relevant channel is column-bound.
  const sizeIsColumn = !!m.size && 'column' in m.size
  const sizes = sizeIsColumn ? new Float32Array(numRows) : undefined
  const sizeCol = sizeIsColumn ? table.getChild((m.size as { column: string }).column) : null

  const useScales = isBoxMapping(m)
  const scaleXIsColumn = !!m.scaleX && 'column' in m.scaleX
  const scaleYIsColumn = !!m.scaleY && 'column' in m.scaleY
  const scaleZIsColumn = !!m.scaleZ && 'column' in m.scaleZ
  const anyScaleColumn = scaleXIsColumn || scaleYIsColumn || scaleZIsColumn
  const scales = useScales && anyScaleColumn ? new Float32Array(numRows * 3) : undefined
  const scaleXCol = scaleXIsColumn ? table.getChild((m.scaleX as { column: string }).column) : null
  const scaleYCol = scaleYIsColumn ? table.getChild((m.scaleY as { column: string }).column) : null
  const scaleZCol = scaleZIsColumn ? table.getChild((m.scaleZ as { column: string }).column) : null

  const scaleXScalar = m.scaleX && 'scalar' in m.scaleX ? m.scaleX.scalar : DEFAULT_BOX_SCALE[0]
  const scaleYScalar = m.scaleY && 'scalar' in m.scaleY ? m.scaleY.scalar : DEFAULT_BOX_SCALE[1]
  const scaleZScalar = m.scaleZ && 'scalar' in m.scaleZ ? m.scaleZ.scalar : DEFAULT_BOX_SCALE[2]

  // Color is always materialized into the per-instance buffer.
  const colorsRGBA = new Uint8Array(numRows * 4)
  const colorIsColumn = !!m.color && 'column' in m.color
  const colorScalar = m.color && 'scalar' in m.color ? (m.color.scalar as number) : DEFAULT_RGBA
  const colorCol = colorIsColumn ? table.getChild((m.color as { column: string }).column) : null

  for (let i = 0; i < numRows; i++) {
    const x = Number(xCol.get(i) ?? NaN)
    const y = Number(yCol.get(i) ?? NaN)
    const z = Number(zCol.get(i) ?? NaN)
    if (!Number.isFinite(x) || !Number.isFinite(y) || !Number.isFinite(z)) {
      return {
        ok: false,
        error: `Row ${i}: non-finite coordinate (x=${x}, y=${y}, z=${z}). Filter NaN/null values in your SQL.`,
      }
    }
    const pBase = i * 3
    positions[pBase] = x
    positions[pBase + 1] = y
    positions[pBase + 2] = z

    if (sizes && sizeCol) {
      const v = Number(sizeCol.get(i) ?? NaN)
      if (!Number.isFinite(v)) {
        return {
          ok: false,
          error: `Row ${i}: non-finite value in column '${(m.size as { column: string }).column}' for channel 'size'.`,
        }
      }
      sizes[i] = v
    }

    if (scales) {
      const sBase = i * 3
      let sx: number
      if (scaleXIsColumn && scaleXCol) {
        sx = Number(scaleXCol.get(i) ?? NaN)
      } else {
        sx = scaleXScalar
      }
      let sy: number
      if (scaleYIsColumn && scaleYCol) {
        sy = Number(scaleYCol.get(i) ?? NaN)
      } else {
        sy = scaleYScalar
      }
      let sz: number
      if (scaleZIsColumn && scaleZCol) {
        sz = Number(scaleZCol.get(i) ?? NaN)
      } else {
        sz = scaleZScalar
      }
      if (!Number.isFinite(sx) || !Number.isFinite(sy) || !Number.isFinite(sz)) {
        return {
          ok: false,
          error: `Row ${i}: non-finite value in scaleX/scaleY/scaleZ (sx=${sx}, sy=${sy}, sz=${sz}).`,
        }
      }
      scales[sBase] = sx
      scales[sBase + 1] = sy
      scales[sBase + 2] = sz
    }

    if (colorIsColumn && colorCol) {
      const v = colorCol.get(i)
      if (v === null || v === undefined) {
        return {
          ok: false,
          error: `Row ${i}: null in column '${(m.color as { column: string }).column}' for channel 'color'.`,
        }
      }
      if (colorColumnKind === 'string') {
        const parsed = rgbaFromHex(String(v))
        if (parsed === null) {
          return {
            ok: false,
            error: `Row ${i}: unparseable color '${String(v)}' in column '${(m.color as { column: string }).column}'. Expected '#rrggbb' or '#rrggbbaa'.`,
          }
        }
        writeRGBA(colorsRGBA, i, parsed)
      } else {
        writeRGBA(colorsRGBA, i, coerceCellToU32(v))
      }
    } else {
      writeRGBA(colorsRGBA, i, colorScalar >>> 0)
    }
  }

  const constants: OverlayConstants = {
    size: m.size && 'scalar' in m.size ? m.size.scalar : DEFAULT_SPHERE_SIZE,
    scale: [scaleXScalar, scaleYScalar, scaleZScalar],
  }

  return {
    ok: true,
    overlay: { table, positions, colorsRGBA, scales, sizes },
    constants,
  }
}

export function materializeRow(table: Table, rowIndex: number): Row {
  const row: Row = {}
  for (const field of table.schema.fields) {
    const col = table.getChild(field.name)
    if (!col) continue
    const value = col.get(rowIndex)
    if (value === null || value === undefined) continue
    row[field.name] = formatArrowValue(value, field.type)
  }
  return row
}
