import { csvParse } from 'd3-dsv'
import { tableFromArrays, tableToIPC } from 'apache-arrow'
import type { Table } from 'apache-arrow'

/**
 * Parse CSV text into an Arrow Table and IPC stream bytes.
 *
 * - Uses d3-dsv for RFC 4180-compliant CSV parsing
 * - Infers column types: Float64 if all non-empty values parse as numbers, otherwise Utf8
 * - Returns both the Arrow Table (for display) and IPC bytes (for WASM registration)
 */
export function csvToArrowIPC(csvText: string): { table: Table; ipcBytes: Uint8Array } {
  const rows = csvParse(csvText.trim())

  if (rows.columns.length === 0) {
    throw new Error('CSV must have at least a header row with column names')
  }

  if (rows.length === 0) {
    throw new Error('CSV must have at least one data row')
  }

  const columns = rows.columns

  // Determine which columns are numeric (all non-empty values parse as numbers)
  const isNumeric = columns.map((col) => {
    return rows.every((row) => {
      const val = row[col]
      if (val === undefined || val === '') return true
      const num = Number(val)
      return !isNaN(num) && isFinite(num)
    })
  })

  // Build typed arrays for each column
  // Arrow's tableFromArrays infers types: Float64Array → Float64, string[] → Utf8/Dictionary
  const arrays: Record<string, Float64Array | string[]> = {}

  for (let i = 0; i < columns.length; i++) {
    const col = columns[i]
    if (isNumeric[i]) {
      const arr = new Float64Array(rows.length)
      for (let r = 0; r < rows.length; r++) {
        const val = rows[r][col]
        arr[r] = val === undefined || val === '' ? NaN : Number(val)
      }
      arrays[col] = arr
    } else {
      const arr: string[] = []
      for (let r = 0; r < rows.length; r++) {
        arr.push(rows[r][col] ?? '')
      }
      arrays[col] = arr
    }
  }

  const table = tableFromArrays(arrays)
  const ipcBytes = tableToIPC(table, 'stream')

  return { table, ipcBytes }
}
