import { csvFormatRows } from 'd3-dsv'
import type { Table } from 'apache-arrow'

export function arrowTableToCsv(table: Table): string {
  const fields = table.schema.fields
  const header = fields.map((f) => f.name)
  const rows: string[][] = []
  for (let i = 0; i < table.numRows; i++) {
    const row = table.get(i)
    rows.push(fields.map((f) => {
      const val = row?.[f.name]
      return val == null ? '' : String(val)
    }))
  }
  return csvFormatRows([header, ...rows])
}

export function triggerCsvDownload(csvContent: string, filename: string): void {
  const blob = new Blob([csvContent], { type: 'text/csv;charset=utf-8' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}
