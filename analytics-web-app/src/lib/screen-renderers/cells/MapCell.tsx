import { useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { Table } from 'apache-arrow'
import type {
  CellTypeMetadata,
  CellRendererProps,
  CellEditorProps,
  CellExecutionContext,
} from '../cell-registry'
import type { QueryCellConfig, CellConfig, CellState } from '../notebook-types'
import { AvailableVariablesPanel } from '@/components/AvailableVariablesPanel'
import { DocumentationLink, QUERY_GUIDE_URL } from '@/components/DocumentationLink'
import { SyntaxEditor } from '@/components/SyntaxEditor'
import { substituteMacros, validateMacros, DEFAULT_SQL } from '../notebook-utils'
import { timestampToDate } from '@/lib/arrow-utils'
import { MapViewer, type MapEvent } from '@/components/map/MapViewer'
import { EventDetailPanel } from '@/components/map/EventDetailPanel'
import { Map as MapIcon } from 'lucide-react'

// =============================================================================
// Data Transformation (Arrow Table → MapEvent[])
// =============================================================================

/** Reserved column names that map to MapEvent fields (not included in properties) */
const RESERVED_COLUMNS = new Set(['time', 'process_id', 'x', 'y', 'z'])

function arrowTableToMapEvents(table: Table): MapEvent[] {
  const result: MapEvent[] = []
  for (let i = 0; i < table.numRows; i++) {
    const row = table.get(i)
    if (!row) continue

    const time = timestampToDate(row.time)
    const x = parseFloat(String(row.x ?? '0'))
    const y = parseFloat(String(row.y ?? '0'))
    const z = parseFloat(String(row.z ?? '0'))

    if (isNaN(x) || isNaN(y) || isNaN(z)) continue

    // Collect all extra columns as generic properties
    const properties: Record<string, string> = {}
    const rowObj = row.toJSON?.() ?? row
    for (const [key, value] of Object.entries(rowObj)) {
      if (!RESERVED_COLUMNS.has(key) && value != null && String(value).trim() !== '') {
        properties[key] = String(value)
      }
    }

    result.push({
      id: `${row.process_id ?? 'unknown'}-${i}`,
      time: time ?? new Date(),
      processId: String(row.process_id ?? ''),
      x,
      y,
      z,
      properties,
    })
  }
  return result
}

// =============================================================================
// Map Catalog (loaded from public/maps/maps.json)
// =============================================================================

interface MapCatalogEntry {
  name: string
  file: string
}

/** Shared promise so multiple cells don't fetch the catalog twice */
let catalogPromise: Promise<MapCatalogEntry[]> | null = null

function fetchMapCatalog(): Promise<MapCatalogEntry[]> {
  if (!catalogPromise) {
    catalogPromise = fetch('/maps/maps.json')
      .then((res) => (res.ok ? res.json() : []))
      .catch(() => [])
  }
  return catalogPromise
}

function useMapCatalog(): MapCatalogEntry[] {
  const [catalog, setCatalog] = useState<MapCatalogEntry[]>([])
  useEffect(() => {
    fetchMapCatalog().then(setCatalog)
  }, [])
  return catalog
}

// =============================================================================
// Renderer Component
// =============================================================================

export function MapCell({ data, status, options }: CellRendererProps) {
  // Transform Arrow data to MapEvent[] (memoized on table reference)
  const events = useMemo(() => {
    const table = data[0]
    if (!table || table.numRows === 0) return []
    return arrowTableToMapEvents(table)
  }, [data])

  // Ephemeral interaction state
  const [selectedEvent, setSelectedEvent] = useState<MapEvent | null>(null)
  const [resetViewTrigger, setResetViewTrigger] = useState(0)

  // Read visual options with defaults
  const mapUrl = options?.mapUrl as string | undefined
  const markerColor = (options?.markerColor as string) ?? '#bf360c'
  const markerSize = (options?.markerSize as number) ?? 10

  const handleSelectEvent = useCallback((event: MapEvent | null) => {
    setSelectedEvent(event)
  }, [])

  // Z resets the view, scoped to the hovered cell so multiple Map cells on a
  // page don't all reset together and typing 'z' in a text/query cell is ignored.
  const containerRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'z' && e.key !== 'Z') return
      if (e.ctrlKey || e.metaKey || e.altKey) return
      const target = e.target as HTMLElement | null
      if (target?.matches('input, textarea, select, [contenteditable="true"]')) return
      if (!containerRef.current?.matches(':hover')) return
      setResetViewTrigger((t) => t + 1)
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])

  if (status === 'loading') {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
        <span className="ml-2 text-theme-text-secondary text-sm">Loading...</span>
      </div>
    )
  }

  if (events.length === 0 && status === 'success') {
    return (
      <div className="flex items-center justify-center h-full text-theme-text-muted text-sm">
        No spatial data available. Query must return columns: time, x, y, z
      </div>
    )
  }

  return (
    <div ref={containerRef} className="relative w-full h-full overflow-hidden">
      <MapViewer
        mapUrl={mapUrl}
        events={events}
        selectedEventId={selectedEvent?.id}
        onSelectEvent={handleSelectEvent}
        markerColor={markerColor}
        markerSize={markerSize}
        resetViewTrigger={resetViewTrigger}
      />

      {selectedEvent && (
        <EventDetailPanel event={selectedEvent} onClose={() => setSelectedEvent(null)} />
      )}
    </div>
  )
}

// =============================================================================
// Editor Component
// =============================================================================

export function MapCellEditor({
  config,
  onChange,
  variables,
  timeRange,
  onRun,
  cellResults,
  cellSelections,
}: CellEditorProps) {
  const mapConfig = config as QueryCellConfig
  const mapCatalog = useMapCatalog()

  const updateSql = useCallback(
    (sql: string) => {
      onChange({ ...mapConfig, sql })
    },
    [mapConfig, onChange]
  )

  const updateOption = useCallback(
    (key: string, value: unknown) => {
      onChange({ ...mapConfig, options: { ...mapConfig.options, [key]: value } })
    },
    [mapConfig, onChange]
  )

  const validationErrors = useMemo(() => {
    return validateMacros(mapConfig.sql, variables, cellResults, cellSelections).errors
  }, [mapConfig.sql, variables, cellResults, cellSelections])

  return (
    <>
      {/* SQL */}
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          SQL Query
        </label>
        <SyntaxEditor
          value={mapConfig.sql}
          onChange={updateSql}
          language="sql"
          placeholder="SELECT time, x, y, z, process_id FROM ..."
          minHeight="120px"
          onRunShortcut={onRun}
        />
      </div>

      {validationErrors.length > 0 && (
        <div className="text-red-400 text-sm space-y-1">
          {validationErrors.map((err, i) => (
            <div key={i}>Warning: {err}</div>
          ))}
        </div>
      )}

      {/* Map Options */}
      <div className="space-y-3">
        <label className="block text-xs font-medium text-theme-text-secondary uppercase">
          Map Options
        </label>

        {/* Map selection */}
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <label className="text-xs text-theme-text-secondary w-24 shrink-0">Map</label>
            <select
              className="flex-1 bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary focus:outline-none focus:border-accent-link"
              value={(mapConfig.options?.mapUrl as string) ?? ''}
              onChange={(e) => updateOption('mapUrl', e.target.value || undefined)}
            >
              <option value="">None (grid only)</option>
              {mapCatalog.map((entry) => (
                <option key={entry.file} value={entry.file}>
                  {entry.name}
                </option>
              ))}
            </select>
          </div>
          <div className="text-xs text-theme-text-muted ml-[calc(6rem+0.5rem)]">
            Register maps in <code className="text-theme-text-secondary">public/maps/maps.json</code>
          </div>
        </div>

        {/* Marker color */}
        <div className="flex items-center gap-2">
          <label className="text-xs text-theme-text-secondary w-24 shrink-0">Marker Color</label>
          <input
            type="color"
            value={(mapConfig.options?.markerColor as string) ?? '#bf360c'}
            onChange={(e) => updateOption('markerColor', e.target.value)}
            className="w-7 h-7 rounded cursor-pointer border border-theme-border bg-transparent"
          />
        </div>

        {/* Marker size */}
        <div className="flex items-center gap-2">
          <label className="text-xs text-theme-text-secondary w-24 shrink-0">Marker Size</label>
          <input
            type="range"
            min="1"
            max="50"
            value={(mapConfig.options?.markerSize as number) ?? 10}
            onChange={(e) => updateOption('markerSize', Number(e.target.value))}
            className="w-32 accent-accent-link"
          />
          <span className="text-xs text-theme-text-muted w-5">
            {(mapConfig.options?.markerSize as number) ?? 10}
          </span>
        </div>
      </div>

      <AvailableVariablesPanel
        variables={variables}
        timeRange={timeRange}
        cellResults={cellResults}
        cellSelections={cellSelections}
      />
      <DocumentationLink url={QUERY_GUIDE_URL} label="Query Guide" />
    </>
  )
}

// =============================================================================
// Cell Type Metadata
// =============================================================================

// eslint-disable-next-line react-refresh/only-export-components
export const mapMetadata: CellTypeMetadata = {
  renderer: MapCell,
  EditorComponent: MapCellEditor,

  label: 'Map',
  icon: <MapIcon />,
  description: '3D map visualization of spatial events',
  showTypeBadge: true,
  defaultHeight: 500,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'map' as const,
    sql: DEFAULT_SQL.map,
    options: {
      markerColor: '#bf360c',
      markerSize: 10,
    },
  }),

  execute: async (
    config: CellConfig,
    { variables, cellResults, cellSelections, timeRange, runQuery, runQueryAs }: CellExecutionContext
  ) => {
    const mapConfig = config as QueryCellConfig
    const sql = substituteMacros(mapConfig.sql, variables, timeRange, cellResults, cellSelections)
    if (runQueryAs) {
      const table = await runQueryAs(sql, config.name, mapConfig.dataSource)
      return { data: [table] }
    }
    const table = await runQuery(sql)
    return { data: [table] }
  },

  getRendererProps: (config: CellConfig, state: CellState) => ({
    data: state.data,
    status: state.status,
    options: { ...(config as QueryCellConfig).options },
  }),
}
