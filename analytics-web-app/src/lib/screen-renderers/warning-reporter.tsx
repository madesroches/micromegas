/* eslint-disable react-refresh/only-export-components */

import { createContext, useCallback, useEffect, useRef, useState } from 'react'
import { AlertTriangle } from 'lucide-react'

export type ReportWarning = (columnKey: string, warning: string) => void

export const WarningReporterContext = createContext<ReportWarning | null>(null)

/**
 * Encapsulates the column-warnings reducer used by table parents
 * (`TableCell`, `TableRenderer`, `TransposedTableCell`).
 *
 * Pass `overridesSource` as the raw `options?.overrides` (or equivalent).
 * The hook hashes the source by content, so callers do NOT need to ensure
 * referential stability — `options?.overrides ?? []` works the same as a
 * memoized ref.
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

  // Content-hash the override list so a fresh array reference with the
  // same shape (the `?? []` fallback case, or a parent re-rendering) does
  // not trigger a reset. JSON.stringify is fine here — overrides are a
  // small array of `{ column, format }` objects.
  const overridesHash = JSON.stringify(overridesSource ?? null)

  // Skip the reset on the *first* render: child `OverrideCell` effects
  // post warnings during the same commit (child effects run before parent
  // effects), and a mount-time reset would clobber them. After that, track
  // the previous hash — comparing against the initial hash forever would
  // leave stale warnings when the override list churns through edits and
  // ends up at a shape equal to its starting one.
  const prevHashRef = useRef(overridesHash)
  useEffect(() => {
    if (overridesHash === prevHashRef.current) return
    prevHashRef.current = overridesHash
    setColumnWarnings(new Map())
  }, [overridesHash])

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
