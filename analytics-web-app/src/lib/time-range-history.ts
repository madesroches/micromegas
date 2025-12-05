// Time range history management using localStorage

const HISTORY_KEY = 'micromegas-time-range-history'
const MAX_HISTORY = 5

export interface TimeRangeHistoryEntry {
  from: string
  to: string
  label: string
  timestamp: number
}

function getStorage(): Storage | null {
  if (typeof window === 'undefined') {
    return null
  }
  return window.localStorage
}

export function getRecentTimeRanges(): TimeRangeHistoryEntry[] {
  const storage = getStorage()
  if (!storage) {
    return []
  }

  try {
    const stored = storage.getItem(HISTORY_KEY)
    if (!stored) {
      return []
    }
    const entries = JSON.parse(stored) as TimeRangeHistoryEntry[]
    return entries.slice(0, MAX_HISTORY)
  } catch {
    return []
  }
}

export function saveTimeRange(from: string, to: string, label: string): void {
  const storage = getStorage()
  if (!storage) {
    return
  }

  try {
    const entries = getRecentTimeRanges()

    // Remove existing entry with same from/to to avoid duplicates
    const filtered = entries.filter((e) => !(e.from === from && e.to === to))

    // Add new entry at the beginning
    const newEntry: TimeRangeHistoryEntry = {
      from,
      to,
      label,
      timestamp: Date.now(),
    }

    const updated = [newEntry, ...filtered].slice(0, MAX_HISTORY)
    storage.setItem(HISTORY_KEY, JSON.stringify(updated))
  } catch {
    // Ignore storage errors
  }
}

export function clearTimeRangeHistory(): void {
  const storage = getStorage()
  if (!storage) {
    return
  }

  try {
    storage.removeItem(HISTORY_KEY)
  } catch {
    // Ignore storage errors
  }
}
