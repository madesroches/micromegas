import { useState, useEffect } from 'react'
import { Clock } from 'lucide-react'
import { getRecentTimeRanges, type TimeRangeHistoryEntry } from '@/lib/time-range-history'
import type { RecentRangesProps } from './types'

export function RecentRanges({ onSelect }: RecentRangesProps) {
  const [recentRanges, setRecentRanges] = useState<TimeRangeHistoryEntry[]>([])

  useEffect(() => {
    setRecentRanges(getRecentTimeRanges())
  }, [])

  if (recentRanges.length === 0) {
    return null
  }

  return (
    <div className="border-t border-theme-border pt-3 mt-3">
      <div className="flex items-center gap-1.5 px-1 py-1 text-xs font-semibold text-theme-text-muted uppercase tracking-wide">
        <Clock className="w-3 h-3" />
        <span>Recently used</span>
      </div>
      <div className="space-y-0.5 mt-1">
        {recentRanges.map((entry) => (
          <button
            key={entry.timestamp}
            onClick={() => onSelect(entry.from, entry.to)}
            className="w-full text-left px-2 py-1.5 text-sm text-theme-text-primary rounded hover:bg-theme-border transition-colors"
          >
            {entry.label}
          </button>
        ))}
      </div>
    </div>
  )
}
