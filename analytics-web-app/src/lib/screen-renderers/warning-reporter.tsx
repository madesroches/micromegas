/* eslint-disable react-refresh/only-export-components */

import { createContext, useCallback, useRef, useState } from 'react'
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

  // Reset during render (the "derive state from props" pattern) rather than
  // in a useEffect. A reset in an effect runs *after* child OverrideCell
  // effects in the same commit (React fires child effects bottom-up before
  // parent ones), so a parent reset would clobber warnings the children just
  // posted — losing the new warning whenever overrides change from one bad
  // format to another bad format. The inline reset triggers an immediate
  // re-render with an empty Map; on that fresh render the children re-evaluate
  // their templates and post any new warnings.
  const prevHashRef = useRef(overridesHash)
  if (overridesHash !== prevHashRef.current) {
    prevHashRef.current = overridesHash
    setColumnWarnings(new Map())
  }

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
