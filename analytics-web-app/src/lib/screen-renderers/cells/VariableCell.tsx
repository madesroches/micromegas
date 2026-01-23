import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { VariableCellConfig, CellConfig, CellState } from '../notebook-types'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { substituteMacros, DEFAULT_SQL } from '../notebook-utils'

// =============================================================================
// Renderer Component
// =============================================================================

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

// =============================================================================
// Editor Component
// =============================================================================

function VariableCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const varConfig = config as VariableCellConfig
  const isCombobox = (varConfig.variableType || 'combobox') === 'combobox'

  return (
    <>
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Variable Type
        </label>
        <select
          value={varConfig.variableType || 'combobox'}
          onChange={(e) =>
            onChange({ ...varConfig, variableType: e.target.value as 'combobox' | 'text' | 'number' })
          }
          className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
        >
          <option value="combobox">Dropdown (from SQL)</option>
          <option value="text">Text Input</option>
          <option value="number">Number Input</option>
        </select>
      </div>

      {isCombobox && (
        <>
          <div>
            <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
              SQL Query
            </label>
            <textarea
              value={varConfig.sql || ''}
              onChange={(e) => onChange({ ...varConfig, sql: e.target.value })}
              className="w-full min-h-[150px] px-3 py-2 bg-app-bg border border-theme-border rounded-md text-theme-text-primary text-sm font-mono focus:outline-none focus:border-accent-link resize-y"
              placeholder="SELECT value, label FROM ..."
            />
          </div>
          <AvailableVariablesPanel variables={variables} timeRange={timeRange} />
        </>
      )}

      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Default Value
        </label>
        <input
          type="text"
          value={varConfig.defaultValue || ''}
          onChange={(e) => onChange({ ...varConfig, defaultValue: e.target.value })}
          className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
          placeholder="Default value"
        />
      </div>
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

// eslint-disable-next-line react-refresh/only-export-components
export const variableMetadata: CellTypeMetadata = {
  renderer: VariableCell,
  EditorComponent: VariableCellEditor,

  label: 'Variable',
  icon: 'V',
  description: 'User input (dropdown, text, number)',
  showTypeBadge: true,
  defaultHeight: 60,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'variable' as const,
    variableType: 'combobox' as const,
    sql: DEFAULT_SQL.variable,
  }),

  execute: async (config: CellConfig, { variables, timeRange, runQuery }: CellExecutionContext) => {
    const varConfig = config as VariableCellConfig

    // Only combobox variables need execution
    if (varConfig.variableType !== 'combobox' || !varConfig.sql) {
      return null // Nothing to execute
    }

    const sql = substituteMacros(varConfig.sql, variables, timeRange)
    const result = await runQuery(sql)

    // Extract options from result
    // Convention: 1 column = value+label, 2 columns = value then label
    const options: { label: string; value: string }[] = []
    if (result && result.numRows > 0 && result.numCols > 0) {
      const schema = result.schema
      const valueColName = schema.fields[0].name
      const labelColName = schema.fields.length > 1 ? schema.fields[1].name : valueColName
      for (let i = 0; i < result.numRows; i++) {
        const row = result.get(i)
        if (row) {
          const value = String(row[valueColName] ?? '')
          const label = String(row[labelColName] ?? value)
          options.push({ label, value })
        }
      }
    }

    return { data: result, variableOptions: options }
  },

  // Auto-select first option if no value is set
  onExecutionComplete: (config: CellConfig, state: CellState, { setVariableValue }) => {
    const options = state.variableOptions
    if (options && options.length > 0) {
      setVariableValue(config.name, options[0].value)
    }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    variableType: (config as VariableCellConfig).variableType,
    variableOptions: state.variableOptions,
  }),
}
