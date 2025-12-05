'use client'

import { useState, useCallback } from 'react'
import { DayPicker } from 'react-day-picker'
import { format, setHours, setMinutes, startOfDay, endOfDay } from 'date-fns'
import { Calendar, Clock } from 'lucide-react'
import 'react-day-picker/style.css'

interface DateTimePickerProps {
  value: Date | undefined
  onChange: (date: Date | undefined) => void
  label?: string
  placeholder?: string
}

export function DateTimePicker({ value, onChange, label, placeholder }: DateTimePickerProps) {
  const [isCalendarOpen, setIsCalendarOpen] = useState(false)

  const hours = value ? value.getHours() : 0
  const minutes = value ? value.getMinutes() : 0

  const handleDateSelect = useCallback(
    (date: Date | undefined) => {
      if (!date) {
        onChange(undefined)
        return
      }
      // Preserve time when changing date
      const newDate = setMinutes(setHours(date, hours), minutes)
      onChange(newDate)
      setIsCalendarOpen(false)
    },
    [onChange, hours, minutes]
  )

  const handleTimeChange = useCallback(
    (type: 'hours' | 'minutes', val: string) => {
      const numVal = parseInt(val, 10)
      if (isNaN(numVal)) return

      const baseDate = value || new Date()
      let newDate: Date

      if (type === 'hours') {
        const clampedHours = Math.max(0, Math.min(23, numVal))
        newDate = setHours(baseDate, clampedHours)
      } else {
        const clampedMinutes = Math.max(0, Math.min(59, numVal))
        newDate = setMinutes(baseDate, clampedMinutes)
      }

      onChange(newDate)
    },
    [value, onChange]
  )

  const handleNow = useCallback(() => {
    onChange(new Date())
  }, [onChange])

  const handleStartOfDay = useCallback(() => {
    const base = value || new Date()
    onChange(startOfDay(base))
  }, [value, onChange])

  const handleEndOfDay = useCallback(() => {
    const base = value || new Date()
    onChange(endOfDay(base))
  }, [value, onChange])

  return (
    <div className="space-y-2">
      {label && (
        <label className="text-xs font-medium text-theme-text-muted uppercase tracking-wide">
          {label}
        </label>
      )}

      <div className="relative">
        <button
          type="button"
          onClick={() => setIsCalendarOpen(!isCalendarOpen)}
          className="w-full flex items-center gap-2 px-3 py-2 bg-app-card border border-theme-border rounded-md text-sm text-theme-text-primary hover:border-theme-border-hover focus:outline-none focus:ring-2 focus:ring-accent-link/50"
        >
          <Calendar className="w-4 h-4 text-theme-text-secondary" />
          <span className="flex-1 text-left">
            {value ? format(value, 'MMM d, yyyy') : placeholder || 'Select date'}
          </span>
        </button>

        {isCalendarOpen && (
          <>
            <div className="fixed inset-0 z-20" onClick={() => setIsCalendarOpen(false)} />
            <div className="absolute left-0 top-full mt-1 z-30 bg-app-panel border border-theme-border rounded-md shadow-lg p-2">
              <DayPicker
                mode="single"
                selected={value}
                onSelect={handleDateSelect}
                className="rdp-custom"
              />
            </div>
          </>
        )}
      </div>

      <div className="flex items-center gap-2">
        <Clock className="w-4 h-4 text-theme-text-secondary" />
        <input
          type="number"
          min={0}
          max={23}
          value={hours.toString().padStart(2, '0')}
          onChange={(e) => handleTimeChange('hours', e.target.value)}
          className="w-14 px-2 py-1.5 bg-app-card border border-theme-border rounded text-sm text-theme-text-primary text-center focus:outline-none focus:ring-2 focus:ring-accent-link/50"
        />
        <span className="text-theme-text-secondary">:</span>
        <input
          type="number"
          min={0}
          max={59}
          value={minutes.toString().padStart(2, '0')}
          onChange={(e) => handleTimeChange('minutes', e.target.value)}
          className="w-14 px-2 py-1.5 bg-app-card border border-theme-border rounded text-sm text-theme-text-primary text-center focus:outline-none focus:ring-2 focus:ring-accent-link/50"
        />
      </div>

      <div className="flex gap-1">
        <button
          type="button"
          onClick={handleNow}
          className="px-2 py-1 text-xs bg-theme-border rounded hover:bg-theme-border-hover text-theme-text-secondary transition-colors"
        >
          Now
        </button>
        <button
          type="button"
          onClick={handleStartOfDay}
          className="px-2 py-1 text-xs bg-theme-border rounded hover:bg-theme-border-hover text-theme-text-secondary transition-colors"
        >
          Start of day
        </button>
        <button
          type="button"
          onClick={handleEndOfDay}
          className="px-2 py-1 text-xs bg-theme-border rounded hover:bg-theme-border-hover text-theme-text-secondary transition-colors"
        >
          End of day
        </button>
      </div>

      <style jsx global>{`
        .rdp-custom {
          --rdp-accent-color: var(--color-accent-link);
          --rdp-background-color: var(--color-app-card);
        }
        .rdp-custom .rdp-day_button {
          color: var(--color-theme-text-primary);
        }
        .rdp-custom .rdp-day_button:hover {
          background-color: var(--color-theme-border);
        }
        .rdp-custom .rdp-selected .rdp-day_button {
          background-color: var(--rdp-accent-color);
          color: white;
        }
        .rdp-custom .rdp-month_caption {
          color: var(--color-theme-text-primary);
        }
        .rdp-custom .rdp-weekday {
          color: var(--color-theme-text-muted);
        }
        .rdp-custom .rdp-button_previous,
        .rdp-custom .rdp-button_next {
          color: var(--color-theme-text-secondary);
        }
        .rdp-custom .rdp-button_previous:hover,
        .rdp-custom .rdp-button_next:hover {
          background-color: var(--color-theme-border);
        }
        .rdp-custom .rdp-outside {
          opacity: 0.4;
        }
      `}</style>
    </div>
  )
}
