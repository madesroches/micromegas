import { useState, useCallback, useEffect, useRef } from 'react'
import { Clock, ChevronDown } from 'lucide-react'
import { saveTimeRange } from '@/lib/time-range-history'
import { parseTimeRange } from '@/lib/time-range'
import { QuickRanges } from './QuickRanges'
import { CustomRange } from './CustomRange'
import { RecentRanges } from './RecentRanges'
import type { TimeRangePickerProps } from './types'

/**
 * TimeRangePicker - controlled component for selecting time ranges.
 *
 * Follows the MVC pattern:
 * - Receives time range values as props (from parent controller)
 * - Dispatches changes via onChange callback
 * - Does not read URL directly
 *
 * @param from - Raw "from" value (relative like "now-1h" or ISO string)
 * @param to - Raw "to" value (relative like "now" or ISO string)
 * @param onChange - Callback when user selects a new time range
 */
export function TimeRangePicker({ from, to, onChange }: TimeRangePickerProps) {
  const [isOpen, setIsOpen] = useState(false)
  const popoverRef = useRef<HTMLDivElement>(null)

  // Derive display label from props
  const parsed = parseTimeRange(from, to)

  const handleSelect = useCallback(
    (newFrom: string, newTo: string) => {
      const newParsed = parseTimeRange(newFrom, newTo)
      saveTimeRange(newFrom, newTo, newParsed.label)
      onChange(newFrom, newTo)
      setIsOpen(false)
    },
    [onChange]
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
          {from === 'now-24h' ? '24h' : from.replace('now-', '')}
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
                  currentFrom={from}
                  currentTo={to}
                  onSelect={handleSelect}
                />
                <CustomRange
                  from={from}
                  to={to}
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

// Re-export types for consumers
export type { TimeRangePickerProps } from './types'
