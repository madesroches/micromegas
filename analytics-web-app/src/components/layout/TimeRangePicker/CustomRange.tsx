'use client'

import { useState, useCallback, useEffect } from 'react'
import { Calendar } from 'lucide-react'
import { isValidTimeExpression, parseRelativeTime, isRelativeTime } from '@/lib/time-range'
import { DateTimePicker } from '@/components/ui/DateTimePicker'
import type { CustomRangeProps } from './types'

export function CustomRange({ from, to, onApply }: CustomRangeProps) {
  const [fromInput, setFromInput] = useState(from)
  const [toInput, setToInput] = useState(to)
  const [fromDate, setFromDate] = useState<Date | undefined>(() => {
    try {
      return parseRelativeTime(from)
    } catch {
      return undefined
    }
  })
  const [toDate, setToDate] = useState<Date | undefined>(() => {
    try {
      return parseRelativeTime(to)
    } catch {
      return undefined
    }
  })
  const [showFromCalendar, setShowFromCalendar] = useState(false)
  const [showToCalendar, setShowToCalendar] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    setFromInput(from)
    setToInput(to)
    try {
      setFromDate(parseRelativeTime(from))
    } catch {
      setFromDate(undefined)
    }
    try {
      setToDate(parseRelativeTime(to))
    } catch {
      setToDate(undefined)
    }
  }, [from, to])

  const handleFromInputChange = useCallback((value: string) => {
    setFromInput(value)
    setError(null)
    if (isValidTimeExpression(value)) {
      try {
        setFromDate(parseRelativeTime(value))
      } catch {
        // Keep the previous date
      }
    }
  }, [])

  const handleToInputChange = useCallback((value: string) => {
    setToInput(value)
    setError(null)
    if (isValidTimeExpression(value)) {
      try {
        setToDate(parseRelativeTime(value))
      } catch {
        // Keep the previous date
      }
    }
  }, [])

  const handleFromDateSelect = useCallback((date: Date | undefined) => {
    if (date) {
      setFromDate(date)
      setFromInput(date.toISOString())
      setShowFromCalendar(false)
      setError(null)
    }
  }, [])

  const handleToDateSelect = useCallback((date: Date | undefined) => {
    if (date) {
      setToDate(date)
      setToInput(date.toISOString())
      setShowToCalendar(false)
      setError(null)
    }
  }, [])

  const handleApply = useCallback(() => {
    if (!isValidTimeExpression(fromInput)) {
      setError('Invalid "From" time expression')
      return
    }
    if (!isValidTimeExpression(toInput)) {
      setError('Invalid "To" time expression')
      return
    }

    // Validate that from is before to
    try {
      const fromDate = parseRelativeTime(fromInput)
      const toDate = parseRelativeTime(toInput)
      if (fromDate >= toDate) {
        setError('"From" must be before "To"')
        return
      }
    } catch {
      setError('Invalid time range')
      return
    }

    setError(null)
    onApply(fromInput, toInput)
  }, [fromInput, toInput, onApply])

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
              onChange={(e) => handleFromInputChange(e.target.value)}
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
              onChange={(e) => handleToInputChange(e.target.value)}
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
