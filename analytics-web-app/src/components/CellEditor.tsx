import { useState, useCallback, useEffect } from 'react'
import { X, Play, Trash2 } from 'lucide-react'
import { CellType } from '@/lib/screen-renderers/cell-registry'
import { Button } from '@/components/ui/button'

const CELL_TYPE_LABELS: Record<CellType, string> = {
  table: 'Table',
  chart: 'Chart',
  log: 'Log',
  markdown: 'Markdown',
  variable: 'Variable',
}

interface CellConfig {
  name: string
  type: CellType
  sql?: string
  content?: string
  options?: Record<string, unknown>
  variableType?: 'combobox' | 'text' | 'number'
  defaultValue?: string
  layout: { height: number | 'auto'; collapsed?: boolean }
}

interface CellEditorProps {
  cell: CellConfig
  variables: Record<string, string>
  timeRange: { begin: string; end: string }
  onClose: () => void
  onUpdate: (updates: Partial<CellConfig>) => void
  onRun: () => void
  onDelete: () => void
}

export function CellEditor({
  cell,
  variables,
  timeRange,
  onClose,
  onUpdate,
  onRun,
  onDelete,
}: CellEditorProps) {
  // Local state for editing
  const [editedSql, setEditedSql] = useState(cell.sql || '')
  const [editedContent, setEditedContent] = useState(cell.content || '')
  const [editedName, setEditedName] = useState(cell.name)

  // Reset local state when cell changes
  useEffect(() => {
    setEditedSql(cell.sql || '')
    setEditedContent(cell.content || '')
    setEditedName(cell.name)
  }, [cell.name, cell.sql, cell.content])

  // Save SQL changes
  const handleSqlChange = useCallback(
    (value: string) => {
      setEditedSql(value)
      onUpdate({ sql: value })
    },
    [onUpdate]
  )

  // Save content changes (for markdown)
  const handleContentChange = useCallback(
    (value: string) => {
      setEditedContent(value)
      onUpdate({ content: value })
    },
    [onUpdate]
  )

  // Save name changes
  const handleNameChange = useCallback(
    (value: string) => {
      setEditedName(value)
      onUpdate({ name: value })
    },
    [onUpdate]
  )

  // Build available variables list
  const availableVars = [
    { name: 'begin', value: timeRange.begin },
    { name: 'end', value: timeRange.end },
    ...Object.entries(variables).map(([name, value]) => ({ name, value })),
  ]

  const showSqlEditor = cell.type !== 'markdown'
  const showContentEditor = cell.type === 'markdown'

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
        <div className="flex items-center gap-2">
          <span className="text-[11px] px-1.5 py-0.5 rounded bg-app-card text-theme-text-secondary uppercase font-medium">
            {CELL_TYPE_LABELS[cell.type]}
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
        {/* Cell Name */}
        <div>
          <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
            Cell Name
          </label>
          <input
            type="text"
            value={editedName}
            onChange={(e) => handleNameChange(e.target.value)}
            className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
          />
        </div>

        {/* SQL Editor */}
        {showSqlEditor && (
          <div>
            <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
              SQL Query
            </label>
            <textarea
              value={editedSql}
              onChange={(e) => handleSqlChange(e.target.value)}
              className="w-full min-h-[150px] px-3 py-2 bg-app-bg border border-theme-border rounded-md text-theme-text-primary text-sm font-mono focus:outline-none focus:border-accent-link resize-y"
              placeholder="SELECT * FROM ..."
            />
          </div>
        )}

        {/* Content Editor (for markdown) */}
        {showContentEditor && (
          <div>
            <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
              Markdown Content
            </label>
            <textarea
              value={editedContent}
              onChange={(e) => handleContentChange(e.target.value)}
              className="w-full min-h-[200px] px-3 py-2 bg-app-bg border border-theme-border rounded-md text-theme-text-primary text-sm font-mono focus:outline-none focus:border-accent-link resize-y"
              placeholder="# Heading\n\nYour markdown here..."
            />
          </div>
        )}

        {/* Variable settings (for variable cells) */}
        {cell.type === 'variable' && (
          <>
            <div>
              <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
                Variable Type
              </label>
              <select
                value={cell.variableType || 'combobox'}
                onChange={(e) => onUpdate({ variableType: e.target.value as 'combobox' | 'text' | 'number' })}
                className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
              >
                <option value="combobox">Dropdown (from SQL)</option>
                <option value="text">Text Input</option>
                <option value="number">Number Input</option>
              </select>
            </div>

            <div>
              <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
                Default Value
              </label>
              <input
                type="text"
                value={cell.defaultValue || ''}
                onChange={(e) => onUpdate({ defaultValue: e.target.value })}
                className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
                placeholder="Default value"
              />
            </div>
          </>
        )}

        {/* Available Variables */}
        {showSqlEditor && availableVars.length > 0 && (
          <div>
            <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
              Available Variables
            </label>
            <div className="bg-app-card rounded-md p-2 text-xs space-y-1">
              {availableVars.map((v) => (
                <div key={v.name} className="flex justify-between py-0.5">
                  <span className="text-accent-link font-mono">${v.name}</span>
                  <span className="text-theme-text-muted truncate ml-2">{v.value}</span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* Footer */}
      <div className="p-3 border-t border-theme-border space-y-2">
        {/* Run button (for cells with SQL) */}
        {showSqlEditor && (
          <Button onClick={onRun} className="w-full gap-2">
            <Play className="w-4 h-4" />
            Run Cell
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
