import {
  rgbaFromHex,
  hexFromRgba,
  packedRgbaToCss,
  coerceCellToU32,
  cellColorToCss,
} from '../color-utils'

describe('rgbaFromHex', () => {
  it('should parse #rrggbb with default alpha ff', () => {
    expect(rgbaFromHex('#ff0000')).toBe(0xff0000ff)
  })

  it('should parse #rrggbbaa with explicit alpha', () => {
    expect(rgbaFromHex('#ff000080')).toBe(0xff000080)
  })

  it('should parse lowercase hex', () => {
    expect(rgbaFromHex('#1565c0ff')).toBe(0x1565c0ff)
  })

  it('should return null for invalid input', () => {
    expect(rgbaFromHex('not a color')).toBeNull()
    expect(rgbaFromHex('#xyz')).toBeNull()
    expect(rgbaFromHex('')).toBeNull()
    expect(rgbaFromHex('#12345')).toBeNull() // 5 hex chars, not 6 or 8
  })

  it('should return null for non-string input', () => {
    expect(rgbaFromHex(123 as never)).toBeNull()
  })
})

describe('hexFromRgba / packedRgbaToCss', () => {
  it('should format u32 as 8-digit hex string', () => {
    expect(hexFromRgba(0xff0000ff)).toBe('#ff0000ff')
  })

  it('should preserve full alpha', () => {
    expect(hexFromRgba(0x1565c0ff)).toBe('#1565c0ff')
  })

  it('should preserve zero alpha', () => {
    expect(hexFromRgba(0xff000000)).toBe('#ff000000')
  })

  it('round-trips #rrggbb (alpha added as ff)', () => {
    const u32 = rgbaFromHex('#bf360c')!
    expect(hexFromRgba(u32)).toBe('#bf360cff')
  })

  it('round-trips #rrggbbaa', () => {
    const u32 = rgbaFromHex('#1565c080')!
    expect(hexFromRgba(u32)).toBe('#1565c080')
  })

  it('packedRgbaToCss is identical to hexFromRgba', () => {
    const u32 = 0xffb300ff
    expect(packedRgbaToCss(u32)).toBe(hexFromRgba(u32))
  })
})

describe('coerceCellToU32', () => {
  it('should pass through a small positive number', () => {
    expect(coerceCellToU32(255)).toBe(255)
  })

  it('should reinterpret signed negative as u32', () => {
    // 0xbf360cff as signed int32 is negative
    const signed = 0xbf360cff | 0 // force signed int32
    const u32 = coerceCellToU32(signed)
    expect(u32).toBe(0xbf360cff)
  })

  it('should handle bigint by masking to 32 bits', () => {
    expect(coerceCellToU32(0xff0000ffn)).toBe(0xff0000ff)
  })

  it('should handle large bigint by masking', () => {
    // Value larger than u32 — mask to lower 32 bits
    expect(coerceCellToU32(0x1ff0000ffn)).toBe(0xff0000ff)
  })
})

describe('cellColorToCss', () => {
  it('should decode integer kind as packed RGBA', () => {
    const result = cellColorToCss(0xff0000ff, 'integer')
    expect(result).toBe('#ff0000ff')
  })

  it('should decode bigint integer kind', () => {
    const result = cellColorToCss(0xff0000ffn, 'integer')
    expect(result).toBe('#ff0000ff')
  })

  it('should decode string kind #rrggbb → #rrggbbff', () => {
    const result = cellColorToCss('#ff0000', 'string')
    expect(result).toBe('#ff0000ff')
  })

  it('should decode string kind #rrggbbaa unchanged', () => {
    const result = cellColorToCss('#ff000080', 'string')
    expect(result).toBe('#ff000080')
  })

  it('should return null for invalid string color', () => {
    const result = cellColorToCss('not-a-color', 'string')
    expect(result).toBeNull()
  })

  it('should decode binary kind [R,G,B,A]', () => {
    const bytes = new Uint8Array([0xff, 0x00, 0x00, 0xff])
    const result = cellColorToCss(bytes, 'binary')
    expect(result).toBe('#ff0000ff')
  })

  it('should return null for binary with wrong length', () => {
    const bytes = new Uint8Array([0xff, 0x00, 0x00])
    const result = cellColorToCss(bytes, 'binary')
    expect(result).toBeNull()
  })
})
