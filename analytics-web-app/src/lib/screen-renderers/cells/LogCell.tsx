import { useMemo } from 'react'
import { CellRendererProps, registerCellRenderer } from '../cell-registry'
import { timestampToDate } from '@/lib/arrow-utils'

const LEVEL_NAMES: Record<number, string> = {
  1: 'FATAL',
  2: 'ERROR',
  3: 'WARN',
  4: 'INFO',
  5: 'DEBUG',
  6: 'TRACE',
}

function formatLocalTime(utcTime: unknown): string {
  if (!utcTime) return ''.padEnd(29)

  const date = timestampToDate(utcTime)
  if (!date) return ''.padEnd(29)

  // Try to extract nanoseconds from string representation
  let nanoseconds = '000000000'
  const str = String(utcTime)
  const nanoMatch = str.match(/\.(\d+)/)
  if (nanoMatch) {
    nanoseconds = nanoMatch[1].padEnd(9, '0').slice(0, 9)
  }

  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  const seconds = String(date.getSeconds()).padStart(2, '0')

  return `${year}-${month}-${day} ${hours}:${minutes}:${seconds}.${nanoseconds}`
}

function getLevelColor(level: string): string {
  switch (level) {
    case 'FATAL':
      return 'text-accent-error-bright'
    case 'ERROR':
      return 'text-accent-error'
    case 'WARN':
      return 'text-accent-warning'
    case 'INFO':
      return 'text-accent-link'
    case 'DEBUG':
      return 'text-theme-text-secondary'
    case 'TRACE':
      return 'text-theme-text-muted'
    default:
      return 'text-theme-text-primary'
  }
}

interface LogRow {
  time: unknown
  level: string
  target: string
  msg: string
}

export function LogCell({ data, status }: CellRendererProps) {
  // Extract rows from data
  const rows = useMemo<LogRow[]>(() => {
    if (!data || data.numRows === 0) return []

    const result: LogRow[] = []
    for (let i = 0; i < Math.min(data.numRows, 100); i++) {
      const row = data.get(i)
      if (row) {
        const levelValue = row.level
        const levelStr =
          typeof levelValue === 'number' ? LEVEL_NAMES[levelValue] || 'UNKNOWN' : String(levelValue ?? '')
        result.push({
          time: row.time,
          level: levelStr,
          target: String(row.target ?? ''),
          msg: String(row.msg ?? ''),
        })
      }
    }
    return result
  }, [data])

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center py-8">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
      </div>
    )
  }

  if (rows.length === 0) {
    return (
      <div className="text-center py-8 text-theme-text-muted text-sm">
        No log entries found
      </div>
    )
  }

  return (
    <div className="overflow-auto h-full bg-app-bg border border-theme-border rounded-md font-mono text-xs">
      {rows.map((row, index) => (
        <div
          key={index}
          className="flex px-3 py-1 border-b border-app-panel hover:bg-app-panel/50 transition-colors"
        >
          <span className="text-theme-text-muted mr-3 w-[188px] min-w-[188px] whitespace-nowrap">
            {formatLocalTime(row.time)}
          </span>
          <span className={`w-[38px] min-w-[38px] mr-3 font-semibold ${getLevelColor(row.level)}`}>
            {row.level}
          </span>
          <span
            className="text-accent-highlight mr-3 w-[200px] min-w-[200px] truncate"
            title={row.target}
          >
            {row.target}
          </span>
          <span className="text-theme-text-primary flex-1 break-words">{row.msg}</span>
        </div>
      ))}
      {data && data.numRows > 100 && (
        <div className="px-3 py-2 text-xs text-theme-text-muted text-center bg-app-card border-t border-theme-border">
          Showing 100 of {data.numRows} entries
        </div>
      )}
    </div>
  )
}

// Register this cell renderer
registerCellRenderer('log', LogCell)
