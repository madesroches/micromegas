import { useState } from 'react'
import { ChevronDown, ChevronRight } from 'lucide-react'

interface RowSelectionEditorProps {
  selectionMode: 'none' | 'single'
  cellName: string
  onChange: (mode: 'none' | 'single') => void
}

export function RowSelectionEditor({
  selectionMode,
  cellName,
  onChange,
}: RowSelectionEditorProps) {
  const [isExpanded, setIsExpanded] = useState(selectionMode !== 'none')

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
          <span className="text-sm font-semibold text-theme-text-primary">Row Selection</span>
          {!isExpanded && (
            <span
              className={`px-1.5 py-0.5 text-xs rounded ${
                selectionMode === 'single'
                  ? 'bg-accent-link/20 text-accent-link'
                  : 'bg-theme-border text-theme-text-muted'
              }`}
            >
              {selectionMode === 'single' ? 'Single' : 'None'}
            </span>
          )}
        </div>
      </button>

      {/* Content */}
      {isExpanded && (
        <div className="px-4 py-3 space-y-3">
          <div className="space-y-2">
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="radio"
                name="selectionMode"
                checked={selectionMode === 'none'}
                onChange={() => onChange('none')}
                className="accent-accent-link"
              />
              <span className="text-sm text-theme-text-primary">None</span>
            </label>
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="radio"
                name="selectionMode"
                checked={selectionMode === 'single'}
                onChange={() => onChange('single')}
                className="accent-accent-link"
              />
              <span className="text-sm text-theme-text-primary">Single</span>
            </label>
          </div>

          {/* Help text */}
          <div className="text-xs text-theme-text-muted space-y-1 pt-2 border-t border-theme-border">
            <div>
              Selected row values are available as:
            </div>
            <div>
              <code className="px-1 py-0.5 bg-theme-border rounded">${cellName}.selected.column</code>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
