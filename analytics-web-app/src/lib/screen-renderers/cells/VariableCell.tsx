import { CellRendererProps, registerCellRenderer } from '../cell-registry'

export function VariableCell({
  value,
  onValueChange,
  variableType,
  variableOptions,
  status,
}: CellRendererProps) {
  const currentValue = value || ''
  const type = variableType || 'text'

  if (status === 'loading') {
    return (
      <div className="flex items-center gap-3 py-1">
        <div className="animate-spin rounded-full h-4 w-4 border-2 border-accent-link border-t-transparent" />
        <span className="text-theme-text-muted text-sm">Loading options...</span>
      </div>
    )
  }

  const handleChange = (newValue: string) => {
    onValueChange?.(newValue)
  }

  return (
    <div className="flex items-center gap-3 py-1">
      {type === 'combobox' && (
        <select
          value={currentValue}
          onChange={(e) => handleChange(e.target.value)}
          className="flex-1 max-w-[400px] px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
        >
          {variableOptions && variableOptions.length > 0 ? (
            variableOptions.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {opt.label}
              </option>
            ))
          ) : (
            <option value="">No options available</option>
          )}
        </select>
      )}

      {type === 'text' && (
        <input
          type="text"
          value={currentValue}
          onChange={(e) => handleChange(e.target.value)}
          className="flex-1 max-w-[400px] px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
          placeholder="Enter value..."
        />
      )}

      {type === 'number' && (
        <input
          type="number"
          value={currentValue}
          onChange={(e) => handleChange(e.target.value)}
          className="flex-1 max-w-[200px] px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
          placeholder="0"
        />
      )}
    </div>
  )
}

// Register this cell renderer
registerCellRenderer('variable', VariableCell)
