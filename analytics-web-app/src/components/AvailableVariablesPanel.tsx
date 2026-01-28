interface AvailableVariablesPanelProps {
  variables: Record<string, string>
  timeRange: { begin: string; end: string }
  /** Additional variables to display (e.g., $order_by for table cells) */
  additionalVariables?: { name: string; description: string }[]
}

export function AvailableVariablesPanel({
  variables,
  timeRange,
  additionalVariables,
}: AvailableVariablesPanelProps) {
  const availableVars = [
    { name: 'begin', value: timeRange.begin },
    { name: 'end', value: timeRange.end },
    ...Object.entries(variables).map(([name, value]) => ({ name, value })),
    ...(additionalVariables ?? []).map((v) => ({ name: v.name, value: v.description })),
  ]

  if (availableVars.length === 0) return null

  return (
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
  )
}
