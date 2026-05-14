import type { Table } from 'apache-arrow'
import { isNumericType, unwrapDictionary } from '@/lib/arrow-utils'
import { formatArrowValue } from '@/lib/screen-renderers/notebook-utils'

/** A single row of the SQL result, formatted to strings for template substitution. */
export type Row = Record<string, string>

export interface Overlay {
  table: Table
  /** Flat [x0,y0,z0, x1,y1,z1, ...] in row order. Length = numRows * 3.
   *  All values are finite — rows with non-finite x/y/z fail the build with
   *  `OverlayResult.ok = false`, so the consumer never sees a partially
   *  populated buffer. */
  positions: Float32Array
}

export type OverlayResult =
  | { ok: true; overlay: Overlay }
  | { ok: false; error: string }

const REQUIRED_COLUMNS = ['x', 'y', 'z'] as const

export function buildOverlay(table: Table): OverlayResult {
  const missing: string[] = []
  for (const name of REQUIRED_COLUMNS) {
    if (!table.getChild(name)) missing.push(name)
  }
  if (missing.length > 0) {
    const available = table.schema.fields.map((f) => f.name).join(', ')
    return {
      ok: false,
      error: `Missing required columns: ${missing.join(', ')}. Available: ${available}`,
    }
  }

  for (const name of REQUIRED_COLUMNS) {
    const field = table.schema.fields.find((f) => f.name === name)!
    if (!isNumericType(unwrapDictionary(field.type))) {
      return {
        ok: false,
        error: `Column '${name}' must be numeric, got ${field.type.toString()}`,
      }
    }
  }

  const xCol = table.getChild('x')!
  const yCol = table.getChild('y')!
  const zCol = table.getChild('z')!
  const numRows = table.numRows
  const positions = new Float32Array(numRows * 3)

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
    const base = i * 3
    positions[base] = x
    positions[base + 1] = y
    positions[base + 2] = z
  }

  return { ok: true, overlay: { table, positions } }
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
