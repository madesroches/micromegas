'use client'

import { useState } from 'react'
import { Clock, ChevronDown } from 'lucide-react'
import { useTimeRange } from '@/hooks/useTimeRange'
import { TIME_RANGE_PRESETS } from '@/lib/time-range'

export function TimeRangeSelector() {
  const { parsed, setPreset, timeRange } = useTimeRange()
  const [isOpen, setIsOpen] = useState(false)

  const handlePresetClick = (preset: string) => {
    setPreset(preset)
    setIsOpen(false)
  }

  return (
    <div className="relative">
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="flex items-center gap-2 px-3 py-1.5 bg-[#2f3540] rounded-l-md text-sm text-gray-200 hover:bg-[#3d4450] transition-colors"
      >
        <Clock className="w-4 h-4 text-gray-400" />
        <span>{parsed.label}</span>
        <ChevronDown className="w-3 h-3 text-gray-400" />
      </button>

      {isOpen && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setIsOpen(false)} />
          <div className="absolute right-0 mt-2 w-64 bg-[#1a1f26] rounded-md shadow-lg border border-[#2f3540] z-20">
            <div className="py-2">
              <div className="px-3 py-1.5 text-xs font-semibold text-gray-500 uppercase tracking-wide">
                Relative time ranges
              </div>
              {TIME_RANGE_PRESETS.map((preset) => (
                <button
                  key={preset.value}
                  onClick={() => handlePresetClick(preset.value)}
                  className={`w-full text-left px-3 py-2 text-sm hover:bg-[#2f3540] transition-colors ${
                    timeRange.from === preset.value && timeRange.to === 'now'
                      ? 'text-blue-400 bg-[#22272e]'
                      : 'text-gray-300'
                  }`}
                >
                  {preset.label}
                </button>
              ))}
            </div>
          </div>
        </>
      )}
    </div>
  )
}
