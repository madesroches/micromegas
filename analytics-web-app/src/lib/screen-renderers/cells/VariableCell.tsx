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
import { evaluateVariableExpression } from '../notebook-expression-eval'

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
// Shared hook for variable input state (debounce, combobox resolution)
// =============================================================================

function useVariableInput({
  value,
  onValueChange,
  variableType,
  variableOptions,
}: Pick<CellRendererProps, 'value' | 'onValueChange' | 'variableType' | 'variableOptions'>) {
  const type = variableType || 'text'

  const [localValue, setLocalValue] = useState<string | undefined>(undefined)
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    setLocalValue(undefined)
  }, [value])

  useEffect(() => {
    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current)
      }
    }
  }, [])

  const handleTextChange = (newValue: string) => {
    setLocalValue(newValue)
    if (timeoutRef.current) {
      clearTimeout(timeoutRef.current)
    }
    timeoutRef.current = setTimeout(() => {
      onValueChange?.(newValue)
    }, 300)
  }

  const handleComboboxChange = (serializedValue: string) => {
    const option = variableOptions?.find(
      (opt) => serializeVariableValue(opt.value) === serializedValue
    )
    if (option) {
      onValueChange?.(option.value)
    } else {
      onValueChange?.(serializedValue)
    }
  }

  const stringValue = getVariableString(value ?? '')
  const displayValue = type === 'text'
    ? (localValue ?? stringValue)
    : serializeVariableValue(value ?? '')

  return { type, localValue, stringValue, displayValue, handleTextChange, handleComboboxChange }
}

// =============================================================================
// Shared options list (used by both renderers)
// =============================================================================

function VariableOptions({ variableOptions }: Pick<CellRendererProps, 'variableOptions'>) {
  if (variableOptions && variableOptions.length > 0) {
    return variableOptions.map((opt) => {
      const serialized = serializeVariableValue(opt.value)
      return (
        <option key={serialized} value={serialized}>
          {opt.label}
        </option>
      )
    })
  }
  return <option value="">No options available</option>
}

// =============================================================================
// Body renderer (unused — variable cells render via titleBarRenderer in the
// cell header; the body is only uncollapsed to show CellContainer error state)
// =============================================================================

// eslint-disable-next-line @typescript-eslint/no-unused-vars
export function VariableCell(_props: CellRendererProps) {
  return null
}

// =============================================================================
// Title Bar Component (compact input for cell header)
// =============================================================================

export function VariableTitleBarContent(props: CellRendererProps) {
  const { type, localValue, stringValue, displayValue, handleTextChange, handleComboboxChange } =
    useVariableInput(props)

  if (props.status === 'loading') {
    return (
      <div className="flex items-center gap-2">
        <div className="animate-spin rounded-full h-3 w-3 border-2 border-accent-link border-t-transparent" />
        <span className="text-theme-text-muted text-xs">Loading...</span>
      </div>
    )
  }

  return (
    <div className="flex items-center">
      {type === 'combobox' && (
        <select
          value={displayValue}
          onChange={(e) => handleComboboxChange(e.target.value)}
          className="w-full max-w-[300px] px-2 py-1 bg-app-card border border-theme-border rounded text-theme-text-primary text-xs focus:outline-none focus:border-accent-link"
        >
          <VariableOptions variableOptions={props.variableOptions} />
        </select>
      )}

      {type === 'text' && (
        <input
          type="text"
          value={localValue ?? stringValue}
          onChange={(e) => handleTextChange(e.target.value)}
          className="w-full max-w-[300px] px-2 py-1 bg-app-card border border-theme-border rounded text-theme-text-primary text-xs focus:outline-none focus:border-accent-link"
          placeholder="Enter value..."
        />
      )}

      {type === 'expression' && (
        <span className="px-2 py-1 text-theme-text-primary text-xs font-mono">
          {stringValue || '(not yet computed)'}
        </span>
      )}
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

function VariableCellEditor({ config, onChange, variables, timeRange }: CellEditorProps) {
  const varConfig = config as VariableCellConfig
  const variableType = varConfig.variableType || 'combobox'
  const isCombobox = variableType === 'combobox'
  const isExpression = variableType === 'expression'

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
          value={variableType}
          onChange={(e) =>
            onChange({ ...varConfig, variableType: e.target.value as 'combobox' | 'text' | 'expression' })
          }
          className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
        >
          <option value="combobox">Dropdown (from SQL)</option>
          <option value="text">Text Input</option>
          <option value="expression">JavaScript Expression</option>
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

      {isExpression && (
        <>
          <div>
            <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
              Expression
            </label>
            <input
              type="text"
              value={varConfig.expression || ''}
              onChange={(e) => onChange({ ...varConfig, expression: e.target.value })}
              className="w-full px-3 py-2 bg-app-card border border-theme-border rounded-md text-theme-text-primary text-sm font-mono focus:outline-none focus:border-accent-link"
              placeholder="snap_interval($duration_ms / $innerWidth)"
            />
          </div>
          <div className="text-xs text-theme-text-muted space-y-1">
            <div>
              Bindings: <code className="text-theme-text-primary">$begin</code>, <code className="text-theme-text-primary">$end</code>,{' '}
              <code className="text-theme-text-primary">$duration_ms</code>,{' '}
              <code className="text-theme-text-primary">$innerWidth</code>,{' '}
              <code className="text-theme-text-primary">$devicePixelRatio</code>,{' '}
              upstream <code className="text-theme-text-primary">$variables</code>
            </div>
            <div>
              Operations: <code className="text-theme-text-primary">snap_interval()</code>,{' '}
              <code className="text-theme-text-primary">Math.*</code>,{' '}
              <code className="text-theme-text-primary">new Date()</code>,{' '}
              arithmetic (<code className="text-theme-text-primary">+ - * / %</code>)
            </div>
          </div>
        </>
      )}

      {!isExpression && (
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
      )}
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

// eslint-disable-next-line react-refresh/only-export-components
export const variableMetadata: CellTypeMetadata = {
  renderer: VariableCell,
  titleBarRenderer: VariableTitleBarContent,
  EditorComponent: VariableCellEditor,

  label: 'Variable',
  icon: 'V',
  description: 'User input (dropdown, text, expression)',
  showTypeBadge: true,
  defaultHeight: 0,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'variable' as const,
    variableType: 'combobox' as const,
    sql: DEFAULT_SQL.variable,
  }),

  execute: async (config: CellConfig, { variables, timeRange, runQuery }: CellExecutionContext) => {
    const varConfig = config as VariableCellConfig

    // Expression variables: evaluate expression and set variable value
    if (varConfig.variableType === 'expression') {
      if (!varConfig.expression) {
        return null
      }
      const begin = timeRange.begin
      const end = timeRange.end
      const result = evaluateVariableExpression(varConfig.expression, {
        begin,
        end,
        durationMs: new Date(end).getTime() - new Date(begin).getTime(),
        innerWidth: window.innerWidth,
        devicePixelRatio: window.devicePixelRatio,
        variables,
      })
      return { data: null, expressionResult: result }
    }

    // Only combobox variables need SQL execution
    if (varConfig.variableType !== 'combobox' || !varConfig.sql) {
      return null // Nothing to execute
    }

    const sql = substituteMacros(varConfig.sql, variables, timeRange)
    const result = await runQuery(sql)

    // Extract options from result with multi-column support
    // Convention:
    // - 1 column: value is string, label is same as value
    // - 2+ columns: value is entire row (object), label is formatted from all values
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
  // For expression variables, set the computed value
  onExecutionComplete: (config: CellConfig, state: CellState, { setVariableValue, currentValue }) => {
    const varConfig = config as VariableCellConfig

    // Expression variables: set the computed value
    if (varConfig.variableType === 'expression') {
      if (state.expressionResult !== undefined) {
        setVariableValue(config.name, state.expressionResult)
      }
      return
    }

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
