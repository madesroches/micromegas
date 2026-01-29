import { useState, useCallback } from 'react'
import { ChevronDown, ChevronRight, Plus, X } from 'lucide-react'
import type { ColumnOverride } from '@/lib/screen-renderers/table-utils'

interface OverrideEditorProps {
  overrides: ColumnOverride[]
  availableColumns: string[]
  onChange: (overrides: ColumnOverride[]) => void
}

export function OverrideEditor({ overrides, availableColumns, onChange }: OverrideEditorProps) {
  const [isExpanded, setIsExpanded] = useState(overrides.length > 0)

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
          {overrides.map((override, index) => (
            <div key={index} className="p-3 bg-app-card rounded-md border border-theme-border">
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
                className="w-full px-2 py-1.5 text-sm bg-app-bg border border-theme-border rounded text-theme-text-primary mb-2"
              >
                {availableColumns.map((col) => (
                  <option key={col} value={col}>
                    {col}
                  </option>
                ))}
              </select>
              <label className="block text-xs font-medium text-theme-text-secondary mb-1">Format</label>
              <textarea
                value={override.format}
                onChange={(e) => handleFormatChange(index, e.target.value)}
                className="w-full px-2 py-1.5 text-sm bg-app-bg border border-theme-border rounded text-theme-text-primary font-mono resize-none"
                rows={2}
                placeholder="[View](/path?id=$row.column_name)"
              />
            </div>
          ))}

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
