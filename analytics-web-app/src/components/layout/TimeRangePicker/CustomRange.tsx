import { useState, useEffect, useMemo } from 'react'
import { Calendar } from 'lucide-react'
import {
  isValidTimeExpression,
  parseRelativeTime,
  formatDateTimeLocal,
} from '@/lib/time-range'
import { DateTimePicker } from '@/components/ui/DateTimePicker'
import type { CustomRangeProps } from './types'

export function CustomRange({ from, to, onApply }: CustomRangeProps) {
  const [fromInput, setFromInput] = useState(from)
  const [toInput, setToInput] = useState(to)
  const [showFromCalendar, setShowFromCalendar] = useState(false)
  const [showToCalendar, setShowToCalendar] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Derive dates from input strings on demand
  const fromDate = useMemo(() => {
    try {
      return parseRelativeTime(fromInput)
    } catch {
      return undefined
    }
  }, [fromInput])

  const toDate = useMemo(() => {
    try {
      return parseRelativeTime(toInput)
    } catch {
      return undefined
    }
  }, [toInput])

  useEffect(() => {
    setFromInput(from)
    setToInput(to)
  }, [from, to])

  const handleFromDateSelect = (date: Date | undefined) => {
    if (date) {
      setFromInput(formatDateTimeLocal(date))
      setShowFromCalendar(false)
      setError(null)
    }
  }

  const handleToDateSelect = (date: Date | undefined) => {
    if (date) {
      setToInput(formatDateTimeLocal(date))
      setShowToCalendar(false)
      setError(null)
    }
  }

  const handleApply = () => {
    // Parse and validate in one step
    let parsedFrom: Date
    let parsedTo: Date
    try {
      parsedFrom = parseRelativeTime(fromInput)
    } catch {
      setError('Invalid "From" time expression')
      return
    }
    try {
      parsedTo = parseRelativeTime(toInput)
    } catch {
      setError('Invalid "To" time expression')
      return
    }

    if (parsedFrom >= parsedTo) {
      setError('"From" must be before "To"')
      return
    }

    setError(null)
    onApply(fromInput, toInput)
  }

  return (
    <div className="flex-1 space-y-4">
      <div className="space-y-3">
        <div>
          <label className="block text-xs font-medium text-theme-text-muted uppercase tracking-wide mb-1.5">
            From
          </label>
          <div className="flex gap-2">
            <input
              type="text"
              value={fromInput}
              onChange={(e) => { setFromInput(e.target.value); setError(null) }}
              placeholder="now-1h or ISO date"
              className={`flex-1 px-3 py-2 bg-app-card border rounded-md text-sm text-theme-text-primary placeholder-theme-text-muted focus:outline-none focus:ring-2 focus:ring-accent-link/50 ${
                fromInput && !isValidTimeExpression(fromInput)
                  ? 'border-status-error'
                  : 'border-theme-border'
              }`}
            />
            <button
              type="button"
              onClick={() => setShowFromCalendar(!showFromCalendar)}
              className="px-2.5 py-2 bg-theme-border rounded-md hover:bg-theme-border-hover transition-colors"
              title="Select from calendar"
            >
              <Calendar className="w-4 h-4 text-theme-text-secondary" />
            </button>
          </div>
          {showFromCalendar && (
            <div className="mt-2">
              <DateTimePicker value={fromDate} onChange={handleFromDateSelect} />
            </div>
          )}
        </div>

        <div>
          <label className="block text-xs font-medium text-theme-text-muted uppercase tracking-wide mb-1.5">
            To
          </label>
          <div className="flex gap-2">
            <input
              type="text"
              value={toInput}
              onChange={(e) => { setToInput(e.target.value); setError(null) }}
              placeholder="now or ISO date"
              className={`flex-1 px-3 py-2 bg-app-card border rounded-md text-sm text-theme-text-primary placeholder-theme-text-muted focus:outline-none focus:ring-2 focus:ring-accent-link/50 ${
                toInput && !isValidTimeExpression(toInput)
                  ? 'border-status-error'
                  : 'border-theme-border'
              }`}
            />
            <button
              type="button"
              onClick={() => setShowToCalendar(!showToCalendar)}
              className="px-2.5 py-2 bg-theme-border rounded-md hover:bg-theme-border-hover transition-colors"
              title="Select from calendar"
            >
              <Calendar className="w-4 h-4 text-theme-text-secondary" />
            </button>
          </div>
          {showToCalendar && (
            <div className="mt-2">
              <DateTimePicker value={toDate} onChange={handleToDateSelect} />
            </div>
          )}
        </div>
      </div>

      {error && <p className="text-sm text-status-error">{error}</p>}

      <button
        type="button"
        onClick={handleApply}
        className="w-full py-2 px-4 bg-accent-link text-white rounded-md text-sm font-medium hover:bg-accent-link/90 transition-colors focus:outline-none focus:ring-2 focus:ring-accent-link/50"
      >
        Apply time range
      </button>

      <div className="text-xs text-theme-text-muted">
        <p>Relative expressions: now, now-1h, now-30m, now-7d</p>
        <p>Supported units: s (seconds), m (minutes), h (hours), d (days), w (weeks)</p>
      </div>
    </div>
  )
}
