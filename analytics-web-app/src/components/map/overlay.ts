import type { DataType, Table } from 'apache-arrow'
import {
  isBinaryType,
  isIntegerType,
  isNumericType,
  isStringType,
  unwrapDictionary,
} from '@/lib/arrow-utils'
import { substituteMacrosRaw } from '@/lib/screen-renderers/notebook-utils'
import type { VariableValue } from '@/lib/screen-renderers/notebook-types'

export type Shape = 'sphere' | 'box'

/**
 * One channel of an overlay mapping: either a literal value to apply to every
 * row (`scalar`), or the name of a column to read per-row values from (`column`).
 *
 * The `scalar` may also be a raw `string` — for numeric channels (size/scale/
 * color), the string is user-entered text that may contain `$var` macros and
 * is resolved to a number by `resolveMappingScalars`. Legacy notebooks store
 * `scalar` as the typed value (`number` for size/scale, RGBA u32 for color);
 * both forms load.
 */
export type ChannelBinding<T = number> =
  | { column: string }
  | { scalar: T | string }

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
   * binding may reference an integer column (read bit-for-bit as u32),
   * a string column (parsed as '#rrggbb' or '#rrggbbaa'), or a 4-byte
   * binary column (treated as packed R,G,B,A — matches what DataFusion
   * produces for a `0xrrggbbaa` hex literal).
   */
  color?: ChannelBinding
}

export interface Overlay {
  table: Table
  /** Flat [x0,y0,z0, x1,y1,z1, ...] in row order. Length = numRows * 3. */
  positions: Float32Array
  /** Per-instance RGBA bytes — [r,g,b,a, r,g,b,a, ...]. Length = numRows * 4.
   *  Allocated iff `color` is column-bound. When absent, the renderer fills
   *  its runtime buffer from `constants.color`. Splitting scalar vs column
   *  here keeps scalar color changes (alpha slider drags) from invalidating
   *  the overlay reference and triggering a full O(numRows) re-layout. */
  colorsRGBA?: Uint8Array
  /** Non-uniform per-instance scale [sx0,sy0,sz0, ...]. Length = numRows * 3.
   *  Allocated iff any of scaleX/scaleY/scaleZ is column-bound (box only).
   *  Only the slots whose channel is column-bound (per `scaleColumnMask`)
   *  are authoritative — the renderer must fall back to `constants.scale[k]`
   *  for any channel whose mask bit is false, otherwise scalar edits would
   *  read stale bake-time values. */
  scales?: Float32Array
  /** Per-channel mask matching `scales`: `[scaleXIsColumn, scaleYIsColumn,
   *  scaleZIsColumn]`. Present iff `scales` is present. */
  scaleColumnMask?: [boolean, boolean, boolean]
  /** Uniform per-instance scale [s0,s1,...]. Length = numRows. Allocated iff
   *  `size` is column-bound (sphere only). When absent, the renderer reads
   *  `constants.size` for every instance. */
  sizes?: Float32Array
}

/** Scalar fallbacks for size and color channels — read by the renderer when
 *  the corresponding per-row buffer is absent. */
export interface OverlayConstants {
  /** Sphere fallback when `overlay.sizes` is absent. */
  size: number
  /** Box fallback when `overlay.scales` is absent. [sx, sy, sz]. */
  scale: [number, number, number]
  /** RGBA u32 fallback when `overlay.colorsRGBA` is absent. */
  color: number
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

/** Narrows a (possibly templated) scalar binding to a number. Falls back to
 *  `fallback` for column-bound channels, missing bindings, or any scalar that
 *  isn't already a finite number. `resolveMappingScalars` is expected to have
 *  converted any string scalars upstream — this is the runtime safety net so
 *  buildOverlay/resolveOverlayConstants can't silently coerce a `"$mySize"`
 *  string into NaN. */
function numericScalarOr(
  b: ChannelBinding | undefined,
  fallback: number,
): number {
  if (!b || !('scalar' in b)) return fallback
  return typeof b.scalar === 'number' && Number.isFinite(b.scalar)
    ? b.scalar
    : fallback
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

  // Validate color column (integer, string, or 4-byte binary).
  let colorColumnKind: 'integer' | 'string' | 'binary' | null = null
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
    } else if (isBinaryType(innerType)) {
      // DataFusion parses `0xrrggbbaa` SQL literals as Binary, not Int. The 4
      // bytes are already in R,G,B,A order — we copy them in directly.
      colorColumnKind = 'binary'
    } else {
      return {
        ok: false,
        error: `Column '${colorColumnName}' for channel 'color' must be integer (packed RGBA, e.g. 0xff0000ff), string ('#rrggbb' or '#rrggbbaa', e.g. '#1565c0'), or 4-byte binary, got ${field.type.toString()}`,
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

  const scaleColumnMask: [boolean, boolean, boolean] | undefined = scales
    ? [scaleXIsColumn, scaleYIsColumn, scaleZIsColumn]
    : undefined

  const scaleXScalar = numericScalarOr(m.scaleX, DEFAULT_BOX_SCALE[0])
  const scaleYScalar = numericScalarOr(m.scaleY, DEFAULT_BOX_SCALE[1])
  const scaleZScalar = numericScalarOr(m.scaleZ, DEFAULT_BOX_SCALE[2])

  // Color: only materialize a per-instance buffer when column-bound. Scalar
  // color flows through `constants.color` so editor scrubbing doesn't
  // invalidate the overlay reference.
  const colorIsColumn = !!m.color && 'column' in m.color
  const colorScalar = numericScalarOr(m.color, DEFAULT_RGBA)
  const colorsRGBA = colorIsColumn ? new Uint8Array(numRows * 4) : undefined
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
      // Only write the column-bound slots. Scalar slots stay at the
      // Float32Array's zero-init value — the renderer ignores them via
      // `scaleColumnMask` and reads `constants.scale[k]` instead, so scalar
      // edits don't get pinned to the bake-time value.
      const sBase = i * 3
      if (scaleXIsColumn && scaleXCol) {
        const sx = Number(scaleXCol.get(i) ?? NaN)
        if (!Number.isFinite(sx)) {
          return {
            ok: false,
            error: `Row ${i}: non-finite value in column '${(m.scaleX as { column: string }).column}' for channel 'scaleX'.`,
          }
        }
        scales[sBase] = sx
      }
      if (scaleYIsColumn && scaleYCol) {
        const sy = Number(scaleYCol.get(i) ?? NaN)
        if (!Number.isFinite(sy)) {
          return {
            ok: false,
            error: `Row ${i}: non-finite value in column '${(m.scaleY as { column: string }).column}' for channel 'scaleY'.`,
          }
        }
        scales[sBase + 1] = sy
      }
      if (scaleZIsColumn && scaleZCol) {
        const sz = Number(scaleZCol.get(i) ?? NaN)
        if (!Number.isFinite(sz)) {
          return {
            ok: false,
            error: `Row ${i}: non-finite value in column '${(m.scaleZ as { column: string }).column}' for channel 'scaleZ'.`,
          }
        }
        scales[sBase + 2] = sz
      }
    }

    if (colorsRGBA && colorCol) {
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
      } else if (colorColumnKind === 'binary') {
        const bytes = v as Uint8Array | ArrayLike<number>
        if (bytes.length !== 4) {
          return {
            ok: false,
            error: `Row ${i}: column '${(m.color as { column: string }).column}' has ${bytes.length} bytes, expected 4 (R,G,B,A).`,
          }
        }
        const base = i * 4
        colorsRGBA[base] = bytes[0]
        colorsRGBA[base + 1] = bytes[1]
        colorsRGBA[base + 2] = bytes[2]
        colorsRGBA[base + 3] = bytes[3]
      } else {
        writeRGBA(colorsRGBA, i, coerceCellToU32(v))
      }
    }
  }

  const constants: OverlayConstants = {
    size: numericScalarOr(m.size, DEFAULT_SPHERE_SIZE),
    scale: [scaleXScalar, scaleYScalar, scaleZScalar],
    color: colorScalar >>> 0,
  }

  return {
    ok: true,
    overlay: { table, positions, colorsRGBA, scales, scaleColumnMask, sizes },
    constants,
  }
}

/**
 * Resolves any string scalars in a mapping into their numeric (or RGBA u32 for
 * color) form. String scalars are user-entered text — possibly containing
 * `$var` macros — and are substituted against the notebook macro context, then
 * parsed. Number scalars pass through unchanged (legacy notebooks). Column
 * bindings pass through unchanged.
 *
 * Returns the resolved mapping on success; on failure returns the first
 * channel that couldn't be resolved (e.g. macro substitution left a value
 * that isn't a finite number, or a color string isn't `#rrggbb[aa]`).
 */
export type MappingResolveResult =
  | { ok: true; mapping: OverlayMapping }
  | { ok: false; error: string }

export interface MappingResolveContext {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
}

/** Fast scan: returns true iff at least one channel has a string scalar that
 *  may need macro substitution. Used to skip the entire resolution pass for
 *  legacy mappings with purely numeric scalars. */
function hasStringScalar(m: OverlayMapping): boolean {
  for (const ch of ['size', 'scaleX', 'scaleY', 'scaleZ', 'color'] as const) {
    const b = m[ch]
    if (b && 'scalar' in b && typeof b.scalar === 'string') return true
  }
  return false
}

export function resolveMappingScalars(
  mapping: OverlayMapping,
  ctx: MappingResolveContext,
): MappingResolveResult {
  // Fast path: legacy mappings store scalars as raw numbers — nothing to
  // resolve. Returning the input by reference lets the caller's memo stay
  // stable across parent re-renders that pass unstable ctx identities.
  if (!hasStringScalar(mapping)) return { ok: true, mapping }

  const resolved: OverlayMapping = { ...mapping }

  // Macro substitution only runs when the scalar text actually contains a
  // `$`. Literal strings like `"10"` or `"#bf360cff"` bypass the regex passes
  // entirely — keeps the hot path cheap when MapCell re-renders per keystroke
  // in an unrelated SQL editor.
  const substitute = (s: string): string =>
    s.includes('$')
      ? substituteMacrosRaw(s, ctx.variables, ctx.timeRange, ctx.cellResults, ctx.cellSelections)
      : s

  const resolveNumeric = (
    label: string,
    b: ChannelBinding | undefined,
  ): { ok: true; binding: ChannelBinding | undefined } | { ok: false; error: string } => {
    if (!b || !('scalar' in b)) return { ok: true, binding: b }
    if (typeof b.scalar === 'number') return { ok: true, binding: b }
    const raw = substitute(b.scalar)
    // Trim+empty check: `Number("")` is 0, which silently turns an unset
    // scalar into zero. Surface it as an error instead so an empty field is
    // visible rather than a render-time mystery.
    const trimmed = raw.trim()
    const num = trimmed === '' ? NaN : Number(trimmed)
    if (!Number.isFinite(num)) {
      return {
        ok: false,
        error: `Channel '${label}': '${b.scalar}' resolved to '${raw}', which is not a number.`,
      }
    }
    return { ok: true, binding: { scalar: num } }
  }

  for (const label of ['size', 'scaleX', 'scaleY', 'scaleZ'] as const) {
    const r = resolveNumeric(label, mapping[label])
    if (!r.ok) return r
    if (r.binding === undefined) {
      delete resolved[label]
    } else {
      resolved[label] = r.binding
    }
  }

  // Color: macro string must resolve to a hex (#rrggbb[aa]) value; reuse
  // rgbaFromHex so the parsing matches what string-typed color columns accept.
  if (mapping.color && 'scalar' in mapping.color) {
    const s = mapping.color.scalar
    if (typeof s === 'string') {
      const raw = substitute(s)
      const rgba = rgbaFromHex(raw)
      if (rgba === null) {
        return {
          ok: false,
          error: `Channel 'color': '${s}' resolved to '${raw}', which is not a #rrggbb or #rrggbbaa hex color.`,
        }
      }
      resolved.color = { scalar: rgba }
    }
  }

  return { ok: true, mapping: resolved }
}

/** Resolves the scalar fallbacks for size and color channels without touching
 *  the table. Cheap to call on every render so cell-level useMemo can key on
 *  scalar values without invalidating the heavyweight `buildOverlay` memo. */
export function resolveOverlayConstants(mapping?: OverlayMapping): OverlayConstants {
  const m = mapping ?? defaultMappingFor('sphere')
  return {
    size: numericScalarOr(m.size, DEFAULT_SPHERE_SIZE),
    scale: [
      numericScalarOr(m.scaleX, DEFAULT_BOX_SCALE[0]),
      numericScalarOr(m.scaleY, DEFAULT_BOX_SCALE[1]),
      numericScalarOr(m.scaleZ, DEFAULT_BOX_SCALE[2]),
    ],
    color: numericScalarOr(m.color, DEFAULT_RGBA) >>> 0,
  }
}

/** Raw column values for one row (no stringification). Null/undefined skipped. */
export function rowValues(table: Table, rowIndex: number): Record<string, unknown> {
  const row: Record<string, unknown> = {}
  for (const field of table.schema.fields) {
    const v = table.getChild(field.name)?.get(rowIndex)
    if (v === null || v === undefined) continue
    row[field.name] = v
  }
  return row
}

/** Column-name → Arrow DataType, for RFC3339 / format_value resolution. */
export function columnTypeMap(table: Table): Map<string, DataType> {
  return new Map(table.schema.fields.map((f) => [f.name, f.type]))
}
