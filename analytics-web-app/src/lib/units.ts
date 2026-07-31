/**
 * Unit normalization and formatting utilities
 *
 * Converts various unit aliases to canonical form for consistent handling.
 *
 * Two invariants a future editor could easily break:
 * - The lookup is **case-sensitive**: UCUM `B` (the bel) and `By` (the byte) are
 *   distinct keys that must not be collapsed by lowercasing.
 * - Canonical rate units are always exactly `<canonical base>/s` (e.g. `kilobytes/s`),
 *   never a bespoke spelling — `splitRate` below depends on that suffix convention.
 */

const HAND_WRITTEN_ALIASES: Record<string, string> = {
  // Time (include canonical names for case-insensitive matching)
  'ns': 'nanoseconds',
  'nanoseconds': 'nanoseconds',
  'Nanoseconds': 'nanoseconds',
  'µs': 'microseconds',
  'us': 'microseconds',
  'microseconds': 'microseconds',
  'Microseconds': 'microseconds',
  'ms': 'milliseconds',
  'milliseconds': 'milliseconds',
  'Milliseconds': 'milliseconds',
  's': 'seconds',
  'seconds': 'seconds',
  'Seconds': 'seconds',
  'min': 'minutes',
  'minutes': 'minutes',
  'Minutes': 'minutes',
  'h': 'hours',
  'hours': 'hours',
  'Hours': 'hours',
  'd': 'days',
  'days': 'days',
  'Days': 'days',
  // Size
  'bytes': 'bytes',
  'Bytes': 'bytes',
  'B': 'bytes',
  'kilobytes': 'kilobytes',
  'Kilobytes': 'kilobytes',
  'KB': 'kilobytes',
  'kb': 'kilobytes',
  'megabytes': 'megabytes',
  'Megabytes': 'megabytes',
  'MB': 'megabytes',
  'gigabytes': 'gigabytes',
  'Gigabytes': 'gigabytes',
  'GB': 'gigabytes',
  'terabytes': 'terabytes',
  'Terabytes': 'terabytes',
  'TB': 'terabytes',
  // Bits (networking, decimal scaling)
  'bit': 'bits',
  'bits': 'bits',
  'Bits': 'bits',
  'kbit': 'kilobits',
  'kbits': 'kilobits',
  'kilobits': 'kilobits',
  'Kilobits': 'kilobits',
  'Mbit': 'megabits',
  'Mbits': 'megabits',
  'megabits': 'megabits',
  'Megabits': 'megabits',
  'Gbit': 'gigabits',
  'Gbits': 'gigabits',
  'gigabits': 'gigabits',
  'Gigabits': 'gigabits',
  'Tbit': 'terabits',
  'Tbits': 'terabits',
  'terabits': 'terabits',
  'Terabits': 'terabits',
  // Rate
  'BytesPerSecond': 'bytes/s',
  'BytesPerSeconds': 'bytes/s',
  'B/s': 'bytes/s',
  'bytes/s': 'bytes/s',
  'bit/s': 'bits/s',
  'bits/s': 'bits/s',
  'bps': 'bits/s',
  // Other
  '%': 'percent',
  'percent': 'percent',
  'deg': 'degrees',
  'degrees': 'degrees',
  'boolean': 'boolean',
  // Temperature (OTel semconv)
  'Cel': 'celsius',
  'celsius': 'celsius',
  // Length (Unreal)
  'cm': 'centimeters',
  'centimeters': 'centimeters',
  // Dimensionless (UCUM unity, Unreal, Rust imetric!)
  '1': '',
  'none': '',
  'None': '',
  'count': '',
  'Count': '',
  'counts': '',
  'units': '',
  'unit': '',
  'iterations': '',
  '1/s': '/s',
  'count/s': '/s',
}

/**
 * UCUM/OTLP codes for scalable units → canonical base name. Each also implies `<code>/s`.
 * Includes the byte/bit spellings already present as bare-form entries in `HAND_WRITTEN_ALIASES`
 * above (`B`, `KB`, `kb`, `MB`, `GB`, `TB`, `bit`, `kbit`, `Mbit`, `Gbit`, `Tbit`, and the
 * spelled-out canonical names) so their `/s` forms are generated here instead of hand-writing a
 * matching rate entry for each one. Re-listing a bare form is harmless — both definitions map to
 * the same canonical name.
 */
const UCUM_SCALED_CODES: Record<string, string> = {
  // Bytes — decimal and binary prefixes both map to the app's 1024-based canonical units.
  'By': 'bytes', 'B': 'bytes', 'bytes': 'bytes',
  'kBy': 'kilobytes', 'KiBy': 'kilobytes', 'KB': 'kilobytes', 'kb': 'kilobytes', 'kilobytes': 'kilobytes',
  'MBy': 'megabytes', 'MiBy': 'megabytes', 'MB': 'megabytes', 'megabytes': 'megabytes',
  'GBy': 'gigabytes', 'GiBy': 'gigabytes', 'GB': 'gigabytes', 'gigabytes': 'gigabytes',
  'TBy': 'terabytes', 'TiBy': 'terabytes', 'TB': 'terabytes', 'terabytes': 'terabytes',
  // Bits — AWS spells megabits/gigabits `MBit`/`GBit` but kilobits/terabits `kbit`/`Tbit`; accept both cases.
  'bit': 'bits', 'bits': 'bits',
  'kBit': 'kilobits', 'kbit': 'kilobits', 'kilobits': 'kilobits',
  'MBit': 'megabits', 'Mbit': 'megabits', 'megabits': 'megabits',
  'GBit': 'gigabits', 'Gbit': 'gigabits', 'gigabits': 'gigabits',
  'TBit': 'terabits', 'Tbit': 'terabits', 'terabits': 'terabits',
}

function expandRates(codes: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {}
  for (const [code, canonical] of Object.entries(codes)) {
    out[code] = canonical
    out[`${code}/s`] = `${canonical}/s`
  }
  return out
}

export const UNIT_ALIASES: Record<string, string> = {
  ...HAND_WRITTEN_ALIASES,
  ...expandRates(UCUM_SCALED_CODES),
}

/** Matches UCUM annotations: free-text in curly braces, e.g. `{Count}`, `{request}`. */
const ANNOTATION_RE = /\{[^}]*\}/g

/**
 * Normalize a unit string to its canonical form.
 *
 * Table lookup runs first, so an explicit alias entry always wins. If the unit
 * carries a UCUM `{...}` annotation and isn't itself a known alias, the
 * annotation is stripped and the result is looked up again (so `{Count}` -> `''`
 * and `By{net}` -> `bytes` without a table entry for every annotated spelling).
 * Returns the original unit if no alias is found.
 */
export function normalizeUnit(unit: string): string {
  const direct = UNIT_ALIASES[unit]
  if (direct !== undefined) return direct
  if (!unit.includes('{')) return unit
  const stripped = unit.replace(ANNOTATION_RE, '')
  return UNIT_ALIASES[stripped] ?? stripped
}

/**
 * Scale-grouping key for a series unit: canonical form, `''` when dimensionless/absent.
 */
export function unitScaleKey(unit: string | undefined | null): string {
  return normalizeUnit(unit ?? '')
}

const RATE_SUFFIX = '/s'

/** Split a canonical unit into its base unit and whether it is a per-second rate. */
function splitRate(normalized: string): { base: string; isRate: boolean } {
  return normalized.endsWith(RATE_SUFFIX)
    ? { base: normalized.slice(0, -RATE_SUFFIX.length), isRate: true }
    : { base: normalized, isRate: false }
}

/**
 * Set of canonical time unit names
 */
export const TIME_UNIT_NAMES = new Set([
  'nanoseconds',
  'microseconds',
  'milliseconds',
  'seconds',
  'minutes',
  'hours',
  'days',
])

/**
 * Set of canonical size unit names
 */
export const SIZE_UNIT_NAMES = new Set([
  'bytes',
  'kilobytes',
  'megabytes',
  'gigabytes',
  'terabytes',
])

/**
 * Check if a unit (or its alias) is a size-based unit.
 * Includes any `<size>/s` rate variant (e.g. `kilobytes/s`) so adaptive scaling
 * works on bandwidth axes.
 */
export function isSizeUnit(unit: string): boolean {
  const { base } = splitRate(normalizeUnit(unit))
  return SIZE_UNIT_NAMES.has(base)
}

/**
 * Set of canonical bit unit names (networking, decimal scaling)
 */
export const BIT_UNIT_NAMES = new Set([
  'bits',
  'kilobits',
  'megabits',
  'gigabits',
  'terabits',
])

/**
 * Check if a unit (or its alias) is a bit-based unit.
 * Includes any `<bit>/s` rate variant (e.g. `kilobits/s`) so adaptive scaling
 * works on bandwidth axes.
 */
export function isBitUnit(unit: string): boolean {
  const { base } = splitRate(normalizeUnit(unit))
  return BIT_UNIT_NAMES.has(base)
}

export type SizeUnit = 'bytes' | 'kilobytes' | 'megabytes' | 'gigabytes' | 'terabytes'

interface SizeUnitInfo {
  unit: SizeUnit
  abbrev: string
  factor: number // multiplier to convert to bytes
}

// Binary size units (power of 2)
const KB = 1024
const MB = KB * 1024
const GB = MB * 1024
const TB = GB * 1024

const SIZE_UNITS: SizeUnitInfo[] = [
  { unit: 'bytes', abbrev: 'B', factor: 1 },
  { unit: 'kilobytes', abbrev: 'KB', factor: KB },
  { unit: 'megabytes', abbrev: 'MB', factor: MB },
  { unit: 'gigabytes', abbrev: 'GB', factor: GB },
  { unit: 'terabytes', abbrev: 'TB', factor: TB },
]

export interface AdaptiveSizeUnit {
  unit: SizeUnit
  abbrev: string
  conversionFactor: number // multiply original value by this to get display value
}

/**
 * Get the unit factor (bytes per unit)
 */
function getSizeUnitFactor(unit: SizeUnit): number {
  const info = SIZE_UNITS.find((u) => u.unit === unit)
  return info?.factor ?? 1
}

/**
 * Convert a value to bytes from any size unit
 */
function toBytes(value: number, unit: SizeUnit): number {
  return value * getSizeUnitFactor(unit)
}

/**
 * Determine the best size unit to display a reference value.
 * Picks a unit where the value falls in a readable range (1-999).
 *
 * @param referenceValue - A representative value (e.g., p99, max) in the original unit
 * @param originalUnit - The original unit of the values (can be an alias)
 * @returns The best unit to use for display
 */
export function getAdaptiveSizeUnit(
  referenceValue: number,
  originalUnit: SizeUnit | string
): AdaptiveSizeUnit {
  const normalized = normalizeUnit(originalUnit)
  const { base, isRate } = splitRate(normalized)
  const baseUnit = base as SizeUnit
  const refBytes = toBytes(referenceValue, baseUnit)

  // Find the best unit where the value is >= 1 (prefer larger units)
  let bestUnit = SIZE_UNITS[0]
  for (let i = SIZE_UNITS.length - 1; i >= 0; i--) {
    const u = SIZE_UNITS[i]
    const valueInUnit = refBytes / u.factor
    if (valueInUnit >= 1) {
      bestUnit = u
      break
    }
  }

  // Calculate the conversion factor from original unit to best unit
  const originalFactor = getSizeUnitFactor(baseUnit)
  const bestFactor = bestUnit.factor
  const conversionFactor = originalFactor / bestFactor

  return {
    unit: bestUnit.unit,
    abbrev: isRate ? bestUnit.abbrev + '/s' : bestUnit.abbrev,
    conversionFactor,
  }
}

const KNOWN_CURRENCY_CODES = new Set<string>(
  typeof Intl.supportedValuesOf === 'function' ? Intl.supportedValuesOf('currency') : []
)

/**
 * Check if a unit is a recognized ISO 4217 currency code (e.g. USD, CAD, EUR).
 * Validated against the runtime's actual currency registry rather than just
 * checking "is this 3 alphabetic characters", since that would also accept
 * non-currency unit abbreviations like `MPH` or `Cel`.
 */
export function isCurrencyUnit(unit: string): boolean {
  return KNOWN_CURRENCY_CODES.has(unit.toUpperCase())
}

/**
 * Format a value as currency using the viewer's runtime locale.
 */
export function formatCurrencyValue(value: number, unit: string): string {
  return new Intl.NumberFormat(undefined, {
    style: 'currency',
    currency: unit.toUpperCase(),
  }).format(value)
}

export type BitUnit = 'bits' | 'kilobits' | 'megabits' | 'gigabits' | 'terabits'

interface BitUnitInfo {
  unit: BitUnit
  abbrev: string
  factor: number // multiplier to convert to bits
}

// Decimal bit units (power of 10, networking convention)
const KBIT = 1000
const MBIT = KBIT * 1000
const GBIT = MBIT * 1000
const TBIT = GBIT * 1000

const BIT_UNITS: BitUnitInfo[] = [
  { unit: 'bits', abbrev: 'bit', factor: 1 },
  { unit: 'kilobits', abbrev: 'kbit', factor: KBIT },
  { unit: 'megabits', abbrev: 'Mbit', factor: MBIT },
  { unit: 'gigabits', abbrev: 'Gbit', factor: GBIT },
  { unit: 'terabits', abbrev: 'Tbit', factor: TBIT },
]

export interface AdaptiveBitUnit {
  unit: BitUnit
  abbrev: string
  conversionFactor: number
}

function getBitUnitFactor(unit: BitUnit): number {
  const info = BIT_UNITS.find((u) => u.unit === unit)
  return info?.factor ?? 1
}

function toBits(value: number, unit: BitUnit): number {
  return value * getBitUnitFactor(unit)
}

/**
 * Determine the best bit unit to display a reference value.
 * Uses decimal scaling (1 kbit = 1000 bits) per networking convention.
 */
export function getAdaptiveBitUnit(
  referenceValue: number,
  originalUnit: BitUnit | string
): AdaptiveBitUnit {
  const normalized = normalizeUnit(originalUnit)
  const { base, isRate } = splitRate(normalized)
  const baseUnit = base as BitUnit
  const refBits = toBits(referenceValue, baseUnit)

  let bestUnit = BIT_UNITS[0]
  for (let i = BIT_UNITS.length - 1; i >= 0; i--) {
    const u = BIT_UNITS[i]
    const valueInUnit = refBits / u.factor
    if (valueInUnit >= 1) {
      bestUnit = u
      break
    }
  }

  const originalFactor = getBitUnitFactor(baseUnit)
  const conversionFactor = originalFactor / bestUnit.factor

  return {
    unit: bestUnit.unit,
    abbrev: isRate ? bestUnit.abbrev + '/s' : bestUnit.abbrev,
    conversionFactor,
  }
}

// Declared after SIZE_UNITS and BIT_UNITS since it derives from them.
const CANONICAL_DISPLAY_ABBREV: Record<string, string> = {
  '': '',
  'percent': '%',
  'degrees': '°',
  'celsius': '°C',
  'centimeters': 'cm',
  // Time — hand-listed rather than imported from time-units.ts's TIME_UNITS, which would create an
  // import cycle (time-units.ts already imports normalizeUnit from this module).
  'nanoseconds': 'ns',
  'microseconds': 'µs',
  'milliseconds': 'ms',
  'seconds': 's',
  'minutes': 'min',
  'hours': 'h',
  'days': 'd',
  // Size/bit — derived from SIZE_UNITS/BIT_UNITS so the abbreviation can never drift from the
  // adaptive-scaling tables; each also gets its `/s` rate form, matching getAdaptiveSizeUnit's/
  // getAdaptiveBitUnit's own `bestUnit.abbrev + '/s'` convention.
  ...Object.fromEntries(SIZE_UNITS.flatMap((u) => [[u.unit, u.abbrev], [`${u.unit}/s`, `${u.abbrev}/s`]])),
  ...Object.fromEntries(BIT_UNITS.flatMap((u) => [[u.unit, u.abbrev], [`${u.unit}/s`, `${u.abbrev}/s`]])),
}

/** The short form shown to users for a canonical unit; falls back to the canonical name itself. */
export function unitDisplayAbbrev(canonicalUnit: string): string {
  return CANONICAL_DISPLAY_ABBREV[canonicalUnit] ?? canonicalUnit
}
