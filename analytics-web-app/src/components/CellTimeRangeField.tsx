import type { TimeRange } from '@/lib/time-range'

interface CellTimeRangeFieldProps {
  value?: TimeRange
  onChange: (value: TimeRange | undefined) => void
}

/**
 * Shared editor field for a cell's optional per-cell query time range
 * override. Emits the full { from, to } object on each keystroke (the
 * caller merges shallowly), or `undefined` once both bounds are cleared so a
 * cleared override doesn't persist an empty object in the saved config.
 */
export function CellTimeRangeField({ value, onChange }: CellTimeRangeFieldProps) {
  const from = value?.from ?? ''
  const to = value?.to ?? ''

  const emit = (nextFrom: string, nextTo: string) => {
    onChange(nextFrom.trim() || nextTo.trim() ? { from: nextFrom, to: nextTo } : undefined)
  }

  return (
    <div>
      <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
        Query Time Range
      </label>
      <div className="space-y-2">
        <div className="flex items-center gap-2">
          <label className="text-xs text-theme-text-secondary w-10 shrink-0">From</label>
          <input
            type="text"
            className="flex-1 bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary placeholder:text-theme-text-muted focus:outline-none focus:border-accent-link"
            placeholder="$from, now-1h, or macro (empty = screen range)"
            value={from}
            onChange={(e) => emit(e.target.value, to)}
          />
        </div>
        <div className="flex items-center gap-2">
          <label className="text-xs text-theme-text-secondary w-10 shrink-0">To</label>
          <input
            type="text"
            className="flex-1 bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary placeholder:text-theme-text-muted focus:outline-none focus:border-accent-link"
            placeholder="$to, now, or macro"
            value={to}
            onChange={(e) => emit(from, e.target.value)}
          />
        </div>
      </div>
      <p className="text-xs text-theme-text-muted mt-1">
        Optional per-cell override. Leave blank to use the screen's global time range.
      </p>
    </div>
  )
}
