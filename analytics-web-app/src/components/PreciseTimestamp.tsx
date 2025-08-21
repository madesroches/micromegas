'use client'

import { useState } from 'react'
import { Copy, Check, Clock, Globe } from 'lucide-react'

interface PreciseTimestampProps {
  timestamp: string // RFC3339 format
  label?: string
  showCopy?: boolean
  showDuration?: boolean
  durationStart?: string // For calculating duration
  className?: string
}

export function PreciseTimestamp({ 
  timestamp, 
  label, 
  showCopy = true, 
  showDuration = false, 
  durationStart,
  className = "" 
}: PreciseTimestampProps) {
  const [copied, setCopied] = useState(false)
  const [showPrecise, setShowPrecise] = useState(false)

  const handleCopy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch (err) {
      console.error('Failed to copy:', err)
    }
  }

  // Parse timestamp and extract nanoseconds
  const parseTimestamp = (ts: string) => {
    try {
      const date = new Date(ts)
      if (isNaN(date.getTime())) return null
      
      // Extract nanosecond part from RFC3339 string
      const match = ts.match(/\.(\d+)Z?$/)
      const fractionalSeconds = match ? match[1] : '0'
      // Pad or truncate to 9 digits for nanoseconds
      const nanoseconds = fractionalSeconds.padEnd(9, '0').slice(0, 9)
      
      return {
        date,
        nanoseconds,
        fullTimestamp: ts
      }
    } catch {
      return null
    }
  }

  const parsed = parseTimestamp(timestamp)
  if (!parsed) {
    return <span className={`text-red-500 ${className}`}>Invalid timestamp</span>
  }

  const { date, fullTimestamp } = parsed


  // Calculate duration if provided
  const duration = showDuration && durationStart ? calculateDuration(durationStart, timestamp) : null

  return (
    <div className={`space-y-2 ${className}`}>
      {label && (
        <div className="text-xs text-gray-600 uppercase font-medium">{label}</div>
      )}
      
      {/* Main display - human readable */}
      <div className="flex items-center gap-2">
        <div className="text-2xl font-bold text-gray-800">
          {date.toLocaleTimeString('en-US', { 
            hour: 'numeric', 
            minute: '2-digit', 
            hour12: true 
          })}
        </div>
        {showCopy && (
          <button
            onClick={() => handleCopy(fullTimestamp)}
            className="p-1 text-gray-400 hover:text-gray-600 transition-colors"
            title="Copy full timestamp"
          >
            {copied ? <Check className="w-4 h-4 text-green-500" /> : <Copy className="w-4 h-4" />}
          </button>
        )}
      </div>

      {/* Date and relative time */}
      <div className="text-sm text-gray-600">
        {date.toLocaleDateString('en-US', { 
          month: 'short', 
          day: 'numeric',
          year: 'numeric'
        })}
        {duration && (
          <span className="ml-2 text-gray-500">
            ({duration})
          </span>
        )}
      </div>

      {/* Precise timestamp toggle */}
      <div className="flex items-center gap-2 text-xs">
        <button
          onClick={() => setShowPrecise(!showPrecise)}
          className="flex items-center gap-1 text-blue-600 hover:text-blue-800 transition-colors"
        >
          <Clock className="w-3 h-3" />
          {showPrecise ? 'Hide' : 'Show'} RFC3339
        </button>
        
        <span className="text-gray-300">|</span>
        
        <div className="flex items-center gap-1 text-gray-500">
          <Globe className="w-3 h-3" />
          Local time
        </div>
      </div>

      {/* RFC3339 timestamp details */}
      {showPrecise && (
        <div className="bg-white border border-gray-200 rounded-lg p-3 shadow-sm">
          <div className="bg-gray-50 rounded px-3 py-2 flex items-center justify-between">
            <span className="font-mono text-xs text-gray-800 break-all flex-1">
              {fullTimestamp}
            </span>
            <button
              onClick={() => handleCopy(fullTimestamp)}
              className="p-1 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded transition-colors ml-2 flex-shrink-0"
              title="Copy RFC3339"
            >
              <Copy className="w-3 h-3" />
            </button>
          </div>
        </div>
      )}
    </div>
  )
}

// Helper function to calculate duration with nanosecond precision
function calculateDuration(startTime: string, endTime: string): string {
  try {
    const start = new Date(startTime).getTime()
    const end = new Date(endTime).getTime()
    const diffMs = end - start

    if (diffMs < 0) return "Invalid duration"

    // Convert to different units
    const seconds = Math.floor(diffMs / 1000)
    const minutes = Math.floor(seconds / 60)
    const hours = Math.floor(minutes / 60)
    const days = Math.floor(hours / 24)

    if (days > 0) {
      const remainingHours = hours % 24
      return `${days}d ${remainingHours}h`
    } else if (hours > 0) {
      const remainingMinutes = minutes % 60
      return `${hours}h ${remainingMinutes}m`
    } else if (minutes > 0) {
      const remainingSeconds = seconds % 60
      return `${minutes}m ${remainingSeconds}s`
    } else {
      return `${seconds}s`
    }
  } catch {
    return "Invalid duration"
  }
}

