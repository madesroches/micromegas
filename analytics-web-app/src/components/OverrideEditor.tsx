import { useState, useCallback, useMemo } from 'react'
import { ChevronDown, ChevronRight, Plus, X, AlertTriangle } from 'lucide-react'
import { type ColumnOverride, validateFormatMacros } from '@/lib/screen-renderers/table-utils'

interface OverrideEditorProps {
  overrides: ColumnOverride[]
  availableColumns: string[]
  /** Variable names available for macro substitution (e.g., from notebook cells) */
  availableVariables?: string[]
  onChange: (overrides: ColumnOverride[]) => void
}

export function OverrideEditor({
  overrides,
  availableColumns,
  availableVariables = [],
  onChange,
}: OverrideEditorProps) {
  const [isExpanded, setIsExpanded] = useState(overrides.length > 0)

  // Skip validation until we have query results (availableColumns is empty while query runs)
  const hasResults = availableColumns.length > 0

  // Validate all overrides for missing column references and unknown macros
  const validationWarnings = useMemo(() => {
    if (!hasResults) {
      return overrides.map(() => ({ missingColumns: [], unknownMacros: [] }))
    }
    return overrides.map((override) =>
      validateFormatMacros(override.format, availableColumns, availableVariables)
    )
  }, [overrides, availableColumns, availableVariables, hasResults])

  // Check which overrides target columns not in the query results
  const availableColumnsSet = useMemo(() => new Set(availableColumns), [availableColumns])
  const isOrphanedColumn = useCallback(
    (column: string) => hasResults && !availableColumnsSet.has(column),
    [availableColumnsSet, hasResults]
  )

  const handleAddOverride = useCallback(() => {
    // Find first column not already overridden
    const usedColumns = new Set(overrides.map((o) => o.column))
    const firstAvailable = availableColumns.find((c) => !usedColumns.has(c)) || availableColumns[0]

    if (firstAvailable) {
      onChange([...overrides, { column: firstAvailable, format: '[Link](/path?id=$row.' + firstAvailable + ')' }])
      setIsExpanded(true)
    }
  }, [overrides, availableColumns, onChange])

  const handleRemoveOverride = useCallback(
    (index: number) => {
      onChange(overrides.filter((_, i) => i !== index))
    },
    [overrides, onChange]
  )

  const handleColumnChange = useCallback(
    (index: number, column: string) => {
      const newOverrides = [...overrides]
      newOverrides[index] = { ...newOverrides[index], column }
      onChange(newOverrides)
    },
    [overrides, onChange]
  )

  const handleFormatChange = useCallback(
    (index: number, format: string) => {
      const newOverrides = [...overrides]
      newOverrides[index] = { ...newOverrides[index], format }
      onChange(newOverrides)
    },
    [overrides, onChange]
  )

  return (
    <div className="border-t border-theme-border">
      {/* Header */}
      <button
        onClick={() => setIsExpanded(!isExpanded)}
        className="w-full flex items-center justify-between px-4 py-2 bg-app-card hover:bg-app-card/80 transition-colors"
      >
        <div className="flex items-center gap-2">
          {isExpanded ? (
            <ChevronDown className="w-4 h-4 text-theme-text-muted" />
          ) : (
            <ChevronRight className="w-4 h-4 text-theme-text-muted" />
          )}
          <span className="text-sm font-semibold text-theme-text-primary">Overrides</span>
          {!isExpanded && overrides.length > 0 && (
            <span className="px-1.5 py-0.5 text-xs bg-accent-link/20 text-accent-link rounded">
              {overrides.length}
            </span>
          )}
        </div>
      </button>

      {/* Content */}
      {isExpanded && (
        <div className="px-4 py-3 space-y-3">
          {overrides.map((override, index) => {
            const isOrphaned = isOrphanedColumn(override.column)
            return (
              <div
                key={index}
                className={`p-3 bg-app-card rounded-md border ${isOrphaned ? 'border-amber-500/50' : 'border-theme-border'}`}
              >
                <div className="flex items-center justify-between mb-2">
                  <label className="text-xs font-medium text-theme-text-secondary">Column</label>
                  <button
                    onClick={() => handleRemoveOverride(index)}
                    className="p-1 text-theme-text-muted hover:text-accent-error transition-colors"
                    title="Remove override"
                  >
                    <X className="w-3.5 h-3.5" />
                  </button>
                </div>
                <select
                  value={override.column}
                  onChange={(e) => handleColumnChange(index, e.target.value)}
                  className={`w-full px-2 py-1.5 text-sm bg-app-bg border rounded text-theme-text-primary mb-2 ${isOrphaned ? 'border-amber-500/50' : 'border-theme-border'}`}
                >
                  {/* Include orphaned column so it's visible and selectable */}
                  {isOrphaned && (
                    <option key={override.column} value={override.column}>
                      {override.column} (not in results)
                    </option>
                  )}
                  {availableColumns.map((col) => (
                    <option key={col} value={col}>
                      {col}
                    </option>
                  ))}
                </select>
                {isOrphaned && (
                  <div className="mb-2 flex items-start gap-1.5 text-xs text-amber-500">
                    <AlertTriangle className="w-3.5 h-3.5 flex-shrink-0 mt-0.5" />
                    <span>Column not in query results. Add it to the SELECT or choose a different column.</span>
                  </div>
                )}
                <label className="block text-xs font-medium text-theme-text-secondary mb-1">Format</label>
                <textarea
                  value={override.format}
                  onChange={(e) => handleFormatChange(index, e.target.value)}
                  className="w-full px-2 py-1.5 text-sm bg-app-bg border border-theme-border rounded text-theme-text-primary font-mono resize-y min-h-[3.5rem]"
                  rows={2}
                  placeholder="[View](/path?id=$row.column_name)"
                />
                {validationWarnings[index]?.unknownMacros.length > 0 && (
                  <div className="mt-1.5 flex items-start gap-1.5 text-xs text-amber-500">
                    <AlertTriangle className="w-3.5 h-3.5 flex-shrink-0 mt-0.5" />
                    <span>
                      Unknown macro{validationWarnings[index].unknownMacros.length > 1 ? 's' : ''}:{' '}
                      {validationWarnings[index].unknownMacros.map((macro, i) => (
                        <span key={macro}>
                          {i > 0 && ', '}
                          <code className="px-1 py-0.5 bg-amber-500/10 rounded">{macro}</code>
                        </span>
                      ))}
                    </span>
                  </div>
                )}
                {validationWarnings[index]?.missingColumns.length > 0 && (
                  <div className="mt-1.5 flex items-start gap-1.5 text-xs text-amber-500">
                    <AlertTriangle className="w-3.5 h-3.5 flex-shrink-0 mt-0.5" />
                    <span>
                      Unknown column{validationWarnings[index].missingColumns.length > 1 ? 's' : ''}:{' '}
                      {validationWarnings[index].missingColumns.map((col, i) => (
                        <span key={col}>
                          {i > 0 && ', '}
                          <code className="px-1 py-0.5 bg-amber-500/10 rounded">{col}</code>
                        </span>
                      ))}
                    </span>
                  </div>
                )}
              </div>
            )
          })}

          {/* Add button */}
          {availableColumns.length > 0 && (
            <button
              onClick={handleAddOverride}
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs text-accent-link hover:text-accent-link/80 transition-colors"
            >
              <Plus className="w-3.5 h-3.5" />
              Add Override
            </button>
          )}

          {/* Help text */}
          <div className="text-xs text-theme-text-muted space-y-1 pt-2 border-t border-theme-border">
            <div>
              Format: <code className="px-1 py-0.5 bg-theme-border rounded">[label](url)</code>
            </div>
            <div>
              Row data: <code className="px-1 py-0.5 bg-theme-border rounded">$row.name</code> or{' '}
              <code className="px-1 py-0.5 bg-theme-border rounded">$row["column-name"]</code>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
