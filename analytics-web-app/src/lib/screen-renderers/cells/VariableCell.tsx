import { useState, useEffect, useRef, useMemo } from 'react'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { VariableCellConfig, CellConfig, CellState, VariableValue } from '../notebook-types'
import {
  getVariableString,
  serializeVariableValue,
} from '../notebook-types'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { DocumentationLink, QUERY_GUIDE_URL } from '@/components/DocumentationLink'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { substituteMacros, validateMacros, DEFAULT_SQL } from '../notebook-utils'

/**
 * Parse a default value string into a VariableValue.
 * Tries to parse as JSON object, otherwise returns as string.
 */
function parseDefaultValue(str: string): VariableValue {
  if (str.startsWith('{')) {
    try {
      const parsed = JSON.parse(str)
      if (typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)) {
        const isValid = Object.values(parsed).every((v) => typeof v === 'string')
        if (isValid) {
          return parsed as Record<string, string>
        }
      }
    } catch {
      // Not valid JSON
    }
  }
  return str
}

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

  const handleComboboxChange = (serializedValue: string) => {
    // Find the option with this serialized value to get the original VariableValue
    const option = variableOptions?.find(
      (opt) => serializeVariableValue(opt.value) === serializedValue
    )
    if (option) {
      onValueChange?.(option.value)
    } else {
      // Fallback: use the serialized value as a string
      onValueChange?.(serializedValue)
    }
  }

  // For text inputs, get the string representation of the value
  const stringValue = getVariableString(value ?? '')
  // Display value: local state (while typing) takes precedence, otherwise use prop
  const displayValue = isTextInput ? (localValue ?? stringValue) : serializeVariableValue(value ?? '')

  return (
    <div className="flex items-center gap-3 py-1">
      {type === 'combobox' && (
        <select
          value={displayValue}
          onChange={(e) => handleComboboxChange(e.target.value)}
          className="flex-1 max-w-[400px] px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
        >
          {variableOptions && variableOptions.length > 0 ? (
            variableOptions.map((opt) => {
              const serialized = serializeVariableValue(opt.value)
              return (
                <option key={serialized} value={serialized}>
                  {opt.label}
                </option>
              )
            })
          ) : (
            <option value="">No options available</option>
          )}
        </select>
      )}

      {type === 'text' && (
        <input
          type="text"
          value={localValue ?? stringValue}
          onChange={(e) => handleTextChange(e.target.value)}
          className="flex-1 max-w-[400px] px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
          placeholder="Enter value..."
        />
      )}

      {type === 'number' && (
        <input
          type="number"
          value={localValue ?? stringValue}
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

  // Validate macro references in SQL (only for combobox type)
  const validationErrors = useMemo(() => {
    if (!isCombobox || !varConfig.sql) return []
    const result = validateMacros(varConfig.sql, variables)
    return result.errors
  }, [isCombobox, varConfig.sql, variables])

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
          {validationErrors.length > 0 && (
            <div className="text-red-400 text-sm space-y-1">
              {validationErrors.map((err, i) => (
                <div key={i}>⚠ {err}</div>
              ))}
            </div>
          )}
          <AvailableVariablesPanel variables={variables} timeRange={timeRange} />
          <DocumentationLink url={QUERY_GUIDE_URL} label="Query Guide" />
        </>
      )}

      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Default Value
        </label>
        <input
          type="text"
          value={varConfig.defaultValue !== undefined ? getVariableString(varConfig.defaultValue) : ''}
          onChange={(e) => {
            const newValue = e.target.value
            // Parse as JSON object if valid, otherwise keep as string
            const parsed = newValue ? parseDefaultValue(newValue) : undefined
            onChange({ ...varConfig, defaultValue: parsed })
          }}
          className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
          placeholder="Default value (or JSON for multi-column)"
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

    // Extract options from result with multi-column support
    // Convention:
    // - 1 column: value is string, label is same as value
    // - 2 columns: value is first column (string), label is second column
    // - 3+ columns: value is entire row (object), label is formatted from all values
    const options: { label: string; value: VariableValue }[] = []
    if (result && result.numRows > 0 && result.numCols > 0) {
      const schema = result.schema
      const columnNames = schema.fields.map((f) => f.name)

      for (let i = 0; i < result.numRows; i++) {
        const row = result.get(i)
        if (!row) continue

        if (columnNames.length === 1) {
          // Single column: store as string
          const val = String(row[columnNames[0]] ?? '')
          options.push({ value: val, label: val })
        } else if (columnNames.length === 2) {
          // Two columns: first is value, second is label (backward compatible)
          const val = String(row[columnNames[0]] ?? '')
          const label = String(row[columnNames[1]] ?? val)
          options.push({ value: val, label })
        } else {
          // Multiple columns: store entire row as object
          const rowObj: Record<string, string> = {}
          for (const col of columnNames) {
            rowObj[col] = String(row[col] ?? '')
          }
          // Label shows all values for display
          const label = Object.values(rowObj).join(' | ')
          options.push({ value: rowObj, label })
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

    // Compare using serialized values for multi-column support
    const currentSerialized = currentValue !== undefined ? serializeVariableValue(currentValue) : undefined

    // If current value exists and is valid, keep it
    if (currentSerialized && options.some((o) => serializeVariableValue(o.value) === currentSerialized)) {
      return
    }

    // Current value is missing or invalid - use default or first option
    // Note: defaultValue is always a string in config, so try to match it
    if (varConfig.defaultValue) {
      const matchingOption = options.find(
        (o) => serializeVariableValue(o.value) === varConfig.defaultValue ||
               getVariableString(o.value) === varConfig.defaultValue
      )
      if (matchingOption) {
        setVariableValue(config.name, matchingOption.value)
        return
      }
    }

    // Fall back to first option
    const firstOption = options[0]?.value
    if (firstOption !== undefined) {
      setVariableValue(config.name, firstOption)
    }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    variableType: (config as VariableCellConfig).variableType,
    variableOptions: state.variableOptions,
  }),
}
