import { useState, useEffect, useRef } from 'react'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { VariableCellConfig, CellConfig, CellState } from '../notebook-types'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { SyntaxEditor } from '@/components/SyntaxEditor'
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
  const type = variableType || 'text'
  const isTextInput = type === 'text' || type === 'number'

  // Local state for text input - allows immediate UI feedback while debouncing the callback
  const [localValue, setLocalValue] = useState<string | undefined>(undefined)
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // When value prop changes externally, clear local state so we show the new value
  useEffect(() => {
    setLocalValue(undefined)
  }, [value])

  // Cleanup timeout on unmount
  useEffect(() => {
    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current)
      }
    }
  }, [])

  const handleTextChange = (newValue: string) => {
    // Update local state immediately for responsive UI
    setLocalValue(newValue)

    // Clear any pending timeout
    if (timeoutRef.current) {
      clearTimeout(timeoutRef.current)
    }

    // Debounce the callback to parent
    timeoutRef.current = setTimeout(() => {
      onValueChange?.(newValue)
    }, 300)
  }

  if (status === 'loading') {
    return (
      <div className="flex items-center gap-3 py-1">
        <div className="animate-spin rounded-full h-4 w-4 border-2 border-accent-link border-t-transparent" />
        <span className="text-theme-text-muted text-sm">Loading options...</span>
      </div>
    )
  }

  const handleComboboxChange = (newValue: string) => {
    onValueChange?.(newValue)
  }

  // Display value: local state (while typing) takes precedence, otherwise use prop
  const displayValue = isTextInput ? (localValue ?? value ?? '') : (value ?? '')

  return (
    <div className="flex items-center gap-3 py-1">
      {type === 'combobox' && (
        <select
          value={displayValue}
          onChange={(e) => handleComboboxChange(e.target.value)}
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
          value={displayValue}
          onChange={(e) => handleTextChange(e.target.value)}
          className="flex-1 max-w-[400px] px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
          placeholder="Enter value..."
        />
      )}

      {type === 'number' && (
        <input
          type="number"
          value={displayValue}
          onChange={(e) => handleTextChange(e.target.value)}
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
            <SyntaxEditor
              value={varConfig.sql || ''}
              onChange={(sql) => onChange({ ...varConfig, sql })}
              language="sql"
              placeholder="SELECT value, label FROM ..."
              minHeight="150px"
            />
          </div>
          <AvailableVariablesPanel variables={variables} timeRange={timeRange} />
          <div className="mt-4">
            <h4 className="text-xs font-semibold uppercase tracking-wide text-theme-text-muted mb-2">
              Documentation
            </h4>
            <a
              href="https://madesroches.github.io/micromegas/docs/query-guide/"
              target="_blank"
              rel="noopener noreferrer"
              className="text-xs text-accent-link hover:underline"
            >
              Query Guide
            </a>
          </div>
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

  // Validate current value or auto-select fallback for combobox variables
  onExecutionComplete: (config: CellConfig, state: CellState, { setVariableValue, currentValue }) => {
    const varConfig = config as VariableCellConfig
    const options = state.variableOptions

    // Only combobox variables need validation
    if (!options || options.length === 0) return

    // If current value exists and is valid, keep it
    if (currentValue && options.some((o) => o.value === currentValue)) {
      return
    }

    // Current value is missing or invalid - use default or first option
    const fallbackValue = varConfig.defaultValue || options[0]?.value
    if (fallbackValue) {
      setVariableValue(config.name, fallbackValue)
    }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    variableType: (config as VariableCellConfig).variableType,
    variableOptions: state.variableOptions,
  }),
}
