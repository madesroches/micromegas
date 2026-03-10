import { useState } from 'react'
import type { Table } from 'apache-arrow'
import type { VariableValue } from '@/lib/screen-renderers/notebook-types'
import { isMultiColumnValue } from '@/lib/screen-renderers/notebook-types'

interface AvailableVariablesPanelProps {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  /** Additional variables to display (e.g., $order_by for table cells) */
  additionalVariables?: { name: string; description: string }[]
  /** Upstream cell result tables (for $cell[N].col references) */
  cellResults: Record<string, Table>
  /** Selected rows from upstream cells (for $cell.selected.col references) */
  cellSelections: Record<string, Record<string, unknown>>
}

function CellSelectionEntry({ cellName, selection }: { cellName: string; selection: Record<string, unknown> }) {
  const [expanded, setExpanded] = useState(false)
  const columns = Object.keys(selection)
  return (
    <div className="space-y-0.5">
      <button
        type="button"
        className="flex justify-between items-center w-full py-0.5 hover:bg-app-hover rounded cursor-pointer text-left"
        onClick={() => setExpanded(!expanded)}
      >
        <span className="text-theme-text-secondary font-mono font-medium">
          <span className="text-theme-text-muted mr-1 inline-block w-3">{expanded ? '▾' : '▸'}</span>
          {cellName}.selected
        </span>
        <span className="text-accent-link text-[10px]">row selected</span>
      </button>
      {expanded &&
        columns.map((col) => (
          <div key={col} className="flex justify-between py-0.5 pl-5">
            <span className="text-accent-link font-mono">
              ${cellName}.selected.{col}
            </span>
            <span className="text-theme-text-muted truncate ml-2 max-w-[120px]">
              {selection[col] != null ? String(selection[col]) : '-'}
            </span>
          </div>
        ))}
    </div>
  )
}

function CellResultEntry({ cellName, table }: { cellName: string; table: Table }) {
  const [expanded, setExpanded] = useState(false)
  return (
    <div className="space-y-0.5">
      <button
        type="button"
        className="flex justify-between items-center w-full py-0.5 hover:bg-app-hover rounded cursor-pointer text-left"
        onClick={() => setExpanded(!expanded)}
      >
        <span className="text-theme-text-secondary font-mono font-medium">
          <span className="text-theme-text-muted mr-1 inline-block w-3">{expanded ? '▾' : '▸'}</span>
          {cellName}
        </span>
        <span className="text-theme-text-muted">
          {table.schema.fields.length} cols, {table.numRows} rows
        </span>
      </button>
      {expanded &&
        table.schema.fields.map((field) => (
          <div key={field.name} className="flex justify-between py-0.5 pl-5">
            <span className="text-accent-link font-mono">
              ${cellName}[0].{field.name}
            </span>
          </div>
        ))}
    </div>
  )
}

export function AvailableVariablesPanel({
  variables,
  timeRange,
  additionalVariables,
  cellResults,
  cellSelections,
}: AvailableVariablesPanelProps) {
  // Time range variables (always simple strings)
  const timeVars = [
    { name: 'from', value: timeRange.begin, isMultiColumn: false },
    { name: 'to', value: timeRange.end, isMultiColumn: false },
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

  // Cell result entries
  const cellResultEntries = Object.entries(cellResults).filter(
    ([, table]) => table.numRows > 0,
  )

  // Cell selection entries
  const cellSelectionEntries = Object.entries(cellSelections)

  const allVars = [...timeVars, ...userVars, ...additionalVars]

  if (allVars.length === 0 && cellResultEntries.length === 0 && cellSelectionEntries.length === 0) return null

  return (
    <div>
      <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
        Available Variables
      </label>
      <div className="bg-app-card rounded-md p-2 text-xs space-y-1">
        {allVars.map((v) => (
          <div key={v.name}>
            {v.isMultiColumn ? (
              // Multi-column variable: show row value and individual columns
              <div className="space-y-0.5">
                <div className="flex justify-between py-0.5">
                  <span className="text-accent-link font-mono">${v.name}</span>
                  <span className="text-theme-text-muted font-mono truncate ml-2 select-all">
                    {JSON.stringify(v.value)}
                  </span>
                </div>
                {Object.entries(v.value as Record<string, string>).map(([col, val]) => (
                  <div key={col} className="flex justify-between py-0.5 pl-2">
                    <span className="text-accent-link font-mono">.{col}</span>
                    <span className="text-theme-text-muted truncate ml-2">{val}</span>
                  </div>
                ))}
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
        {cellResultEntries.length > 0 && allVars.length > 0 && (
          <div className="border-t border-theme-border my-1" />
        )}
        {cellResultEntries.map(([cellName, table]) => (
          <CellResultEntry key={cellName} cellName={cellName} table={table} />
        ))}
        {cellSelectionEntries.length > 0 && (cellResultEntries.length > 0 || allVars.length > 0) && (
          <div className="border-t border-theme-border my-1" />
        )}
        {cellSelectionEntries.map(([cellName, selection]) => (
          <CellSelectionEntry key={cellName} cellName={cellName} selection={selection} />
        ))}
      </div>
    </div>
  )
}
