/**
 * Shared color utilities for chart and map cell color columns.
 * Implements the packed-RGBA u32 convention used by rgba()/color_scale() UDFs.
 */

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
  return (((r << 24) | (g << 16) | (b << 8) | a) >>> 0)
}

/** Format an RGBA u32 as `#rrggbbaa`. */
export function hexFromRgba(rgba: number): string {
  const u = rgba >>> 0
  const hex = u.toString(16).padStart(8, '0')
  return `#${hex}`
}

/** Returns the CSS color string for a packed RGBA u32. */
export const packedRgbaToCss = hexFromRgba

/**
 * Coerce an Arrow column cell into a u32. Int64/UInt64 columns return
 * bigint from col.get(i); smaller integer columns return number
 * (which may be signed-negative for high-bit-set UInt32 — `>>> 0`
 * reinterprets as unsigned without changing the bit pattern).
 */
export function coerceCellToU32(value: unknown): number {
  if (typeof value === 'bigint') {
    return Number(value & 0xffffffffn)
  }
  return (value as number) >>> 0
}

/**
 * Decode a color cell value (from an Arrow color column) to a CSS color string.
 * Returns null for invalid/malformed string input; callers treat null as absent color.
 * - integer: packed RGBA u32 → `#rrggbbaa`
 * - string: `#rrggbb` → `#rrggbbff`, `#rrggbbaa` → unchanged, invalid → null
 * - binary: 4-byte [R,G,B,A] → `#rrggbbaa`
 */
export function cellColorToCss(
  value: unknown,
  kind: 'integer' | 'string' | 'binary',
): string | null {
  if (kind === 'integer') {
    return packedRgbaToCss(coerceCellToU32(value))
  }
  if (kind === 'string') {
    const rgba = rgbaFromHex(String(value))
    if (rgba === null) return null
    return packedRgbaToCss(rgba)
  }
  // binary: 4-byte [R,G,B,A]
  const bytes = value as ArrayLike<number>
  if (!bytes || bytes.length !== 4) return null
  const toHex = (b: number) => (b & 0xff).toString(16).padStart(2, '0')
  return `#${toHex(bytes[0])}${toHex(bytes[1])}${toHex(bytes[2])}${toHex(bytes[3])}`
}
