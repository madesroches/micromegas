import { useState, useCallback, useEffect, useRef } from 'react'
import { Clock, ChevronDown } from 'lucide-react'
import { useTimeRange } from '@/hooks/useTimeRange'
import { saveTimeRange } from '@/lib/time-range-history'
import { parseTimeRange } from '@/lib/time-range'
import { QuickRanges } from './QuickRanges'
import { CustomRange } from './CustomRange'
import { RecentRanges } from './RecentRanges'

export function TimeRangePicker() {
  const { parsed, setTimeRange, timeRange } = useTimeRange()
  const [isOpen, setIsOpen] = useState(false)
  const popoverRef = useRef<HTMLDivElement>(null)

  const handleSelect = useCallback(
    (from: string, to: string) => {
      const newParsed = parseTimeRange(from, to)
      saveTimeRange(from, to, newParsed.label)
      setTimeRange(from, to)
      setIsOpen(false)
    },
    [setTimeRange]
  )

  // Handle keyboard navigation
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && isOpen) {
        setIsOpen(false)
      }
      // Open picker with 't' key when not focused on an input
      if (
        e.key === 't' &&
        !isOpen &&
        document.activeElement?.tagName !== 'INPUT' &&
        document.activeElement?.tagName !== 'TEXTAREA'
      ) {
        e.preventDefault()
        setIsOpen(true)
      }
    }

    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [isOpen])

  // Focus management when popover opens
  useEffect(() => {
    if (isOpen && popoverRef.current) {
      const firstButton = popoverRef.current.querySelector('button')
      firstButton?.focus()
    }
  }, [isOpen])

  return (
    <div className="relative">
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="flex items-center gap-2 px-3 py-1.5 bg-theme-border rounded-l-md text-sm text-theme-text-primary hover:bg-theme-border-hover transition-colors"
        aria-expanded={isOpen}
        aria-haspopup="dialog"
        aria-label={`Time range: ${parsed.label}. Press to change.`}
      >
        <Clock className="w-4 h-4 text-theme-text-secondary" />
        <span className="hidden sm:inline">{parsed.label}</span>
        <span className="sm:hidden">
          {timeRange.from === 'now-24h' ? '24h' : timeRange.from.replace('now-', '')}
        </span>
        <ChevronDown className="w-3 h-3 text-theme-text-secondary" />
      </button>

      {isOpen && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setIsOpen(false)} aria-hidden="true" />
          <div
            ref={popoverRef}
            role="dialog"
            aria-label="Time range picker"
            className="absolute right-0 mt-2 bg-app-panel rounded-md shadow-lg border border-theme-border z-20 w-[520px] max-w-[calc(100vw-2rem)]"
          >
            <div className="p-4">
              <div className="flex gap-4">
                <QuickRanges
                  currentFrom={timeRange.from}
                  currentTo={timeRange.to}
                  onSelect={handleSelect}
                />
                <CustomRange
                  from={timeRange.from}
                  to={timeRange.to}
                  onApply={handleSelect}
                />
              </div>
              <RecentRanges onSelect={handleSelect} />
            </div>
          </div>
        </>
      )}
    </div>
  )
}
