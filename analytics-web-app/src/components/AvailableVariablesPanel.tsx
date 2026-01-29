import type { VariableValue } from '@/lib/screen-renderers/notebook-types'
import { isMultiColumnValue, getVariableString } from '@/lib/screen-renderers/notebook-types'

interface AvailableVariablesPanelProps {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  /** Additional variables to display (e.g., $order_by for table cells) */
  additionalVariables?: { name: string; description: string }[]
}

export function AvailableVariablesPanel({
  variables,
  timeRange,
  additionalVariables,
}: AvailableVariablesPanelProps) {
  // Time range variables (always simple strings)
  const timeVars = [
    { name: 'begin', value: timeRange.begin, isMultiColumn: false },
    { name: 'end', value: timeRange.end, isMultiColumn: false },
  ]

  // User variables (can be simple strings or multi-column objects)
  const userVars = Object.entries(variables).map(([name, value]) => ({
    name,
    value,
    isMultiColumn: isMultiColumnValue(value),
  }))

  // Additional variables (always simple strings)
  const additionalVars = (additionalVariables ?? []).map((v) => ({
    name: v.name,
    value: v.description,
    isMultiColumn: false,
  }))

  const allVars = [...timeVars, ...userVars, ...additionalVars]

  if (allVars.length === 0) return null

  return (
    <div>
      <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
        Available Variables
      </label>
      <div className="bg-app-card rounded-md p-2 text-xs space-y-1">
        {allVars.map((v) => (
          <div key={v.name}>
            {v.isMultiColumn ? (
              // Multi-column variable: show the variable name and all columns
              <div>
                <div className="flex justify-between py-0.5">
                  <span className="text-accent-link font-mono">${v.name}</span>
                  <span className="text-theme-text-muted truncate ml-2">
                    {getVariableString(v.value as VariableValue)}
                  </span>
                </div>
                <div className="ml-4 space-y-0.5">
                  {Object.entries(v.value as Record<string, string>).map(([col, val]) => (
                    <div key={col} className="flex justify-between py-0.5">
                      <span className="text-accent-link/70 font-mono">${v.name}.{col}</span>
                      <span className="text-theme-text-muted truncate ml-2">{val}</span>
                    </div>
                  ))}
                </div>
              </div>
            ) : (
              // Simple string variable
              <div className="flex justify-between py-0.5">
                <span className="text-accent-link font-mono">${v.name}</span>
                <span className="text-theme-text-muted truncate ml-2">{v.value as string}</span>
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  )
}
