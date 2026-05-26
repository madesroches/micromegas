/* eslint-disable react-refresh/only-export-components */

import { createContext, useCallback, useEffect, useRef, useState } from 'react'
import { AlertTriangle } from 'lucide-react'

export type ReportWarning = (columnKey: string, warning: string) => void

export const WarningReporterContext = createContext<ReportWarning | null>(null)

/**
 * Encapsulates the column-warnings reducer used by table parents
 * (`TableCell`, `TableRenderer`, `TransposedTableCell`).
 *
 * Callers MUST pass the *stable source* of overrides (the raw
 * `options?.overrides` or `tableConfig.overrides`) — NOT a locally
 * destructured form like `(options?.overrides as Foo[] | undefined) || []`,
 * whose `|| []` fallback produces a fresh array reference on every render
 * when no overrides are configured. That would re-fire the reset effect
 * every render, schedule a fresh `new Map()`, and trip React's
 * "Too many re-renders" guard.
 */
export function useColumnWarnings(overridesSource: unknown): {
  columnWarnings: Map<string, Set<string>>
  reportWarning: ReportWarning
} {
  const [columnWarnings, setColumnWarnings] = useState<Map<string, Set<string>>>(() => new Map())

  const reportWarning = useCallback<ReportWarning>((columnKey, warning) => {
    setColumnWarnings((prev) => {
      const existing = prev.get(columnKey)
      if (existing?.has(warning)) return prev // dedup; no state churn
      const next = new Map(prev)
      const updated = new Set(existing)
      updated.add(warning)
      next.set(columnKey, updated)
      return next
    })
  }, [])

  // Reset on *changes* to the override list — not on mount. Resetting on
  // mount would clobber warnings that child `OverrideCell` effects post
  // during the same commit (child effects run before parent effects).
  const initialOverridesRef = useRef(overridesSource)
  useEffect(() => {
    if (overridesSource === initialOverridesRef.current) return
    setColumnWarnings(new Map())
  }, [overridesSource])

  return { columnWarnings, reportWarning }
}

interface ColumnHeaderWarningIconProps {
  warnings: string[]
}

/** Amber-tinted icon shown next to a column header when its overrides
 *  produced one or more warnings during this render cycle. */
export function ColumnHeaderWarningIcon({ warnings }: ColumnHeaderWarningIconProps) {
  if (warnings.length === 0) return null
  return (
    <span title={warnings.join('\n')} className="inline-flex items-center">
      <AlertTriangle className="w-3.5 h-3.5 text-amber-300 flex-shrink-0" />
    </span>
  )
}
