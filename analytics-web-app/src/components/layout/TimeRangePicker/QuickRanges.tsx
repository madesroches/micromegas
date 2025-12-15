import { TIME_RANGE_PRESETS } from '@/lib/time-range'
import { Check } from 'lucide-react'
import type { QuickRangesProps } from './types'

export function QuickRanges({ currentFrom, currentTo, onSelect }: QuickRangesProps) {
  const isSelected = (preset: string) => currentFrom === preset && currentTo === 'now'

  return (
    <div className="w-48 border-r border-theme-border pr-3 overflow-y-auto max-h-80">
      <div className="px-1 py-1.5 text-xs font-semibold text-theme-text-muted uppercase tracking-wide">
        Quick ranges
      </div>
      <div className="space-y-0.5">
        {TIME_RANGE_PRESETS.map((preset) => (
          <button
            key={preset.value}
            onClick={() => onSelect(preset.value, 'now')}
            className={`w-full flex items-center justify-between px-2 py-1.5 text-sm rounded hover:bg-theme-border transition-colors ${
              isSelected(preset.value)
                ? 'text-accent-link bg-app-card'
                : 'text-theme-text-primary'
            }`}
          >
            <span>{preset.label}</span>
            {isSelected(preset.value) && <Check className="w-3 h-3" />}
          </button>
        ))}
      </div>
    </div>
  )
}
