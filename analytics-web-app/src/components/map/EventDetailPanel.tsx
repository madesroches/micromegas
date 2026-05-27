import { X } from 'lucide-react'
import type { DataType, Table } from 'apache-arrow'
import { EventDetailContent } from './EventDetailContent'
import type { VariableValue } from '@/lib/screen-renderers/notebook-types'

interface EventDetailPanelProps {
  row: Record<string, unknown>
  columnTypes: Map<string, DataType>
  template: string
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
  onClose: () => void
}

export function EventDetailPanel({
  row,
  columnTypes,
  template,
  variables,
  timeRange,
  cellResults,
  cellSelections,
  onClose,
}: EventDetailPanelProps) {
  return (
    <div className="absolute bottom-4 left-4 w-fit max-w-[50%] max-h-[60%] overflow-auto bg-app-panel border border-theme-border rounded-lg shadow-lg z-10">
      <button
        onClick={onClose}
        className="absolute top-2 right-2 p-1 rounded hover:bg-theme-border transition-colors z-10"
        title="Close"
      >
        <X className="w-4 h-4 text-theme-text-muted" />
      </button>
      <EventDetailContent
        row={row}
        columnTypes={columnTypes}
        template={template}
        variables={variables}
        timeRange={timeRange}
        cellResults={cellResults}
        cellSelections={cellSelections}
      />
    </div>
  )
}
