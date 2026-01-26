import { useState, useCallback, useEffect, useRef } from 'react'
import { Clock, ChevronDown, Copy, ClipboardPaste } from 'lucide-react'
import { saveTimeRange } from '@/lib/time-range-history'
import { parseTimeRange, isValidTimeExpression } from '@/lib/time-range'
import { toast } from '@/lib/use-toast'
import { QuickRanges } from './QuickRanges'
import { CustomRange } from './CustomRange'
import { RecentRanges } from './RecentRanges'
import type { TimeRangePickerProps } from './types'

interface TimeRangeClipboard {
  from: string
  to: string
}

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
  const [showPasteInput, setShowPasteInput] = useState(false)
  const [pasteInputValue, setPasteInputValue] = useState('')
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

  const handleCopy = useCallback(async () => {
    const clipboardData: TimeRangeClipboard = { from, to }
    try {
      await navigator.clipboard.writeText(JSON.stringify(clipboardData))
      toast({
        title: 'Time range copied',
        description: parsed.label,
      })
    } catch {
      toast({
        title: 'Failed to copy',
        description: 'Could not access clipboard',
        variant: 'destructive',
      })
    }
  }, [from, to, parsed.label])

  const applyPastedTimeRange = useCallback(
    (text: string) => {
      if (!text.trim()) {
        toast({
          title: 'Empty input',
          description: 'Please paste a time range JSON',
          variant: 'destructive',
        })
        return false
      }

      let data: TimeRangeClipboard
      try {
        data = JSON.parse(text)
      } catch {
        toast({
          title: 'Invalid format',
          description: 'Expected JSON: {"from":"...","to":"..."}',
          variant: 'destructive',
        })
        return false
      }

      if (typeof data.from !== 'string' || typeof data.to !== 'string') {
        toast({
          title: 'Invalid format',
          description: 'JSON must have "from" and "to" string fields',
          variant: 'destructive',
        })
        return false
      }

      if (!isValidTimeExpression(data.from)) {
        toast({
          title: 'Invalid "from" value',
          description: `"${data.from}" is not a valid time expression`,
          variant: 'destructive',
        })
        return false
      }

      if (!isValidTimeExpression(data.to)) {
        toast({
          title: 'Invalid "to" value',
          description: `"${data.to}" is not a valid time expression`,
          variant: 'destructive',
        })
        return false
      }

      const parsed = parseTimeRange(data.from, data.to)
      if (parsed.from >= parsed.to) {
        toast({
          title: 'Invalid time range',
          description: '"from" must be before "to"',
          variant: 'destructive',
        })
        return false
      }

      handleSelect(data.from, data.to)
      toast({
        title: 'Time range applied',
        description: parsed.label,
      })
      return true
    },
    [handleSelect]
  )

  const handlePaste = useCallback(() => {
    setShowPasteInput(true)
    setPasteInputValue('')
  }, [])


  const handlePasteInputSubmit = useCallback(() => {
    if (applyPastedTimeRange(pasteInputValue)) {
      setShowPasteInput(false)
      setPasteInputValue('')
    }
  }, [pasteInputValue, applyPastedTimeRange])

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
      // Copy time range with Ctrl+Shift+C
      if (e.ctrlKey && e.shiftKey && e.key === 'C') {
        e.preventDefault()
        handleCopy()
      }
      // Paste time range with Ctrl+Shift+V
      if (e.ctrlKey && e.shiftKey && e.key === 'V') {
        e.preventDefault()
        handlePaste()
      }
    }

    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [isOpen, handleCopy, handlePaste])

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
            <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
              <span className="text-sm font-medium text-theme-text-primary">Select Time Range</span>
              <div className="flex items-center gap-1">
                <button
                  type="button"
                  onClick={handleCopy}
                  className="flex items-center gap-1.5 px-2 py-1 text-xs text-theme-text-secondary bg-theme-border rounded hover:bg-theme-border-hover hover:text-theme-text-primary transition-colors"
                  title="Copy time range to clipboard"
                >
                  <Copy className="w-3.5 h-3.5" />
                  Copy
                </button>
                <button
                  type="button"
                  onClick={handlePaste}
                  className="flex items-center gap-1.5 px-2 py-1 text-xs text-theme-text-secondary bg-theme-border rounded hover:bg-theme-border-hover hover:text-theme-text-primary transition-colors"
                  title="Paste time range from clipboard"
                >
                  <ClipboardPaste className="w-3.5 h-3.5" />
                  Paste
                </button>
              </div>
            </div>
            {showPasteInput && (
              <div className="px-4 py-3 border-b border-theme-border bg-app-card">
                <div className="flex gap-2">
                  <input
                    type="text"
                    autoFocus
                    value={pasteInputValue}
                    onChange={(e) => setPasteInputValue(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault()
                        handlePasteInputSubmit()
                      } else if (e.key === 'Escape') {
                        setShowPasteInput(false)
                      }
                    }}
                    placeholder='Paste JSON here: {"from":"now-1h","to":"now"}'
                    className="flex-1 px-3 py-1.5 bg-app-panel border border-theme-border rounded text-sm text-theme-text-primary placeholder-theme-text-muted focus:outline-none focus:ring-2 focus:ring-accent-link/50"
                  />
                  <button
                    type="button"
                    onClick={handlePasteInputSubmit}
                    className="px-3 py-1.5 text-xs font-medium text-white bg-accent-link rounded hover:bg-accent-link/90 transition-colors"
                  >
                    Apply
                  </button>
                  <button
                    type="button"
                    onClick={() => setShowPasteInput(false)}
                    className="px-2 py-1.5 text-xs text-theme-text-secondary bg-theme-border rounded hover:bg-theme-border-hover transition-colors"
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}
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
