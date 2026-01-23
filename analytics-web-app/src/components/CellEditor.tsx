import { useState, useCallback, useEffect } from 'react'
import { X, Play, Trash2 } from 'lucide-react'
import { getCellTypeMetadata } from '@/lib/screen-renderers/cell-registry'
import type { CellConfig } from '@/lib/screen-renderers/notebook-types'
import { validateCellName, sanitizeCellName } from '@/lib/screen-renderers/notebook-utils'
import { Button } from '@/components/ui/button'

interface CellEditorProps {
  cell: CellConfig
  variables: Record<string, string>
  timeRange: { begin: string; end: string }
  existingNames: Set<string>
  onClose: () => void
  onUpdate: (updates: Partial<CellConfig>) => void
  onRun: () => void
  onDelete: () => void
}

export function CellEditor({
  cell,
  variables,
  timeRange,
  existingNames,
  onClose,
  onUpdate,
  onRun,
  onDelete,
}: CellEditorProps) {
  const meta = getCellTypeMetadata(cell.type)

  // Local state for cell name editing
  const [editedName, setEditedName] = useState(cell.name)
  const [nameError, setNameError] = useState<string | null>(null)

  // Reset local state when cell changes
  useEffect(() => {
    setEditedName(cell.name)
    setNameError(null)
  }, [cell.name])

  // Save name changes with validation
  const handleNameChange = useCallback(
    (value: string) => {
      setEditedName(value)

      // Validate the name
      const error = validateCellName(value, existingNames, cell.name)
      if (error) {
        setNameError(error)
        return
      }

      setNameError(null)
      // Sanitize the name for use as an identifier (spaces -> underscores)
      const sanitizedName = sanitizeCellName(value)
      onUpdate({ name: sanitizedName })
    },
    [onUpdate, cell.name, existingNames]
  )

  // Handle config changes from the type-specific editor
  const handleConfigChange = useCallback(
    (newConfig: CellConfig) => {
      onUpdate(newConfig)
    },
    [onUpdate]
  )

  // Determine if this cell can run
  const canRun = !!meta.execute

  return (
    <div className="flex flex-col flex-1 min-h-0">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
        <div className="flex items-center gap-2">
          <span className="text-[11px] px-1.5 py-0.5 rounded bg-app-card text-theme-text-secondary uppercase font-medium">
            {meta.label}
          </span>
          <span className="font-medium text-theme-text-primary truncate">{cell.name}</span>
        </div>
        <button
          onClick={onClose}
          className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors"
          title="Close"
        >
          <X className="w-5 h-5" />
        </button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {/* Cell Name (shared across all cell types) */}
        <div>
          <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
            Cell Name
          </label>
          <input
            type="text"
            value={editedName}
            onChange={(e) => handleNameChange(e.target.value)}
            className={`w-full px-3 py-2 bg-app-card border rounded-md text-theme-text-primary text-sm focus:outline-none ${
              nameError
                ? 'border-accent-error focus:border-accent-error'
                : 'border-theme-border focus:border-accent-link'
            }`}
          />
          {nameError && (
            <p className="mt-1 text-xs text-accent-error">{nameError}</p>
          )}
        </div>

        {/* Type-specific content - each editor decides what to show */}
        <meta.EditorComponent
          config={cell}
          onChange={handleConfigChange}
          variables={variables}
          timeRange={timeRange}
        />
      </div>

      {/* Footer */}
      <div className="p-3 border-t border-theme-border space-y-2">
        {/* Run button (for cells that can execute) */}
        {canRun && (
          <Button onClick={onRun} className="w-full gap-2">
            <Play className="w-4 h-4" />
            Run
          </Button>
        )}

        {/* Delete button */}
        <Button
          variant="outline"
          onClick={onDelete}
          className="w-full gap-2 text-accent-error border-accent-error hover:bg-accent-error/10"
        >
          <Trash2 className="w-4 h-4" />
          Delete Cell
        </Button>
      </div>
    </div>
  )
}
