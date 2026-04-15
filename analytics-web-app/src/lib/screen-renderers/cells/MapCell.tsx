import { useState, useCallback, useMemo, useEffect } from 'react'
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
import { DataSourceSelector } from '@/components/DataSourceSelector'
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

function MapCell({ data, status, options, onOptionsChange }: CellRendererProps) {
  // Transform Arrow data to MapEvent[] (memoized on table reference)
  const events = useMemo(() => {
    const table = data[0]
    if (!table || table.numRows === 0) return []
    return arrowTableToMapEvents(table)
  }, [data])

  // Ephemeral interaction state
  const [selectedEvent, setSelectedEvent] = useState<MapEvent | null>(null)
  const [fitToDataTrigger, setFitToDataTrigger] = useState(0)
  const [resetViewTrigger, setResetViewTrigger] = useState(0)

  // Read visual options with defaults
  const mapUrl = options?.mapUrl as string | undefined
  const showHeatmap = (options?.showHeatmap as boolean) ?? false
  const heatmapRadius = (options?.heatmapRadius as number) ?? 50
  const heatmapIntensity = (options?.heatmapIntensity as number) ?? 0.5
  const markerColor = (options?.markerColor as string) ?? '#bf360c'
  const markerSize = (options?.markerSize as number) ?? 10
  const groundSnap = (options?.groundSnap as boolean) ?? false

  const handleSelectEvent = useCallback((event: MapEvent | null) => {
    setSelectedEvent(event)
  }, [])

  // Allow toggling heatmap/markers from keyboard or future toolbar
  const updateOption = useCallback(
    (key: string, value: unknown) => {
      onOptionsChange({ ...options, [key]: value })
    },
    [options, onOptionsChange]
  )

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
    <div className="relative w-full h-full overflow-hidden">
      {/* Toolbar overlay */}
      <div className="absolute top-2 left-2 z-10 flex items-center gap-2">
        <button
          onClick={() => setFitToDataTrigger((t) => t + 1)}
          className="px-2 py-1 bg-app-panel/90 border border-theme-border rounded text-xs text-theme-text-secondary hover:text-theme-text-primary"
          title="Fit to data"
        >
          Fit
        </button>
        <button
          onClick={() => setResetViewTrigger((t) => t + 1)}
          className="px-2 py-1 bg-app-panel/90 border border-theme-border rounded text-xs text-theme-text-secondary hover:text-theme-text-primary"
          title="Reset view"
        >
          Reset
        </button>
        <button
          onClick={() => updateOption('showHeatmap', !showHeatmap)}
          className={`px-2 py-1 border border-theme-border rounded text-xs transition-colors ${
            showHeatmap
              ? 'bg-accent-link/20 text-accent-link border-accent-link/50'
              : 'bg-app-panel/90 text-theme-text-secondary hover:text-theme-text-primary'
          }`}
          title="Toggle heatmap"
        >
          Heatmap
        </button>
        <span className="text-xs text-theme-text-muted bg-app-panel/90 px-2 py-1 rounded border border-theme-border">
          {events.length.toLocaleString()} events
        </span>
      </div>

      <MapViewer
        mapUrl={mapUrl}
        events={events}
        selectedEventId={selectedEvent?.id}
        onSelectEvent={handleSelectEvent}
        showHeatmap={showHeatmap}
        heatmapRadius={heatmapRadius}
        heatmapIntensity={heatmapIntensity}
        fitToDataTrigger={fitToDataTrigger}
        markerColor={markerColor}
        markerSize={markerSize}
        groundSnap={groundSnap}
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

function MapCellEditor({
  config,
  onChange,
  variables,
  timeRange,
  datasourceVariables,
  defaultDataSource,
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
      {/* Data Source */}
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Data Source
        </label>
        <DataSourceSelector
          value={mapConfig.dataSource || defaultDataSource || ''}
          onChange={(ds) => onChange({ ...mapConfig, dataSource: ds })}
          datasourceVariables={datasourceVariables}
          showNotebookOption={true}
        />
      </div>

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
              <option value="__custom__">Custom URL...</option>
            </select>
          </div>
          {(mapConfig.options?.mapUrl as string) === '__custom__' || (
            mapConfig.options?.mapUrl &&
            !mapCatalog.some((e) => e.file === mapConfig.options?.mapUrl) &&
            mapConfig.options?.mapUrl !== ''
          ) ? (
            <div className="flex items-center gap-2">
              <label className="text-xs text-theme-text-secondary w-24 shrink-0">URL</label>
              <input
                type="text"
                className="flex-1 bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary placeholder:text-theme-text-muted focus:outline-none focus:border-accent-link"
                placeholder="/maps/my-map.glb"
                value={(mapConfig.options?.mapUrl as string) === '__custom__' ? '' : (mapConfig.options?.mapUrl as string) ?? ''}
                onChange={(e) => updateOption('mapUrl', e.target.value || undefined)}
              />
            </div>
          ) : null}
          <div className="text-xs text-theme-text-muted ml-[calc(6rem+0.5rem)]">
            Register maps in <code className="text-theme-text-secondary">public/maps/maps.json</code>
          </div>
        </div>

        {/* Heatmap toggle */}
        <div className="flex items-center gap-2">
          <label className="text-xs text-theme-text-secondary w-24 shrink-0">Heatmap</label>
          <input
            type="checkbox"
            checked={(mapConfig.options?.showHeatmap as boolean) ?? false}
            onChange={(e) => updateOption('showHeatmap', e.target.checked)}
          />
        </div>

        {/* Heatmap radius */}
        {(mapConfig.options?.showHeatmap as boolean) && (
          <>
            <div className="flex items-center gap-2">
              <label className="text-xs text-theme-text-secondary w-24 shrink-0">Radius</label>
              <input
                type="range"
                min="20"
                max="100"
                value={(mapConfig.options?.heatmapRadius as number) ?? 50}
                onChange={(e) => updateOption('heatmapRadius', Number(e.target.value))}
                className="w-32 accent-accent-link"
              />
              <span className="text-xs text-theme-text-muted w-5">
                {(mapConfig.options?.heatmapRadius as number) ?? 50}
              </span>
            </div>
            <div className="flex items-center gap-2">
              <label className="text-xs text-theme-text-secondary w-24 shrink-0">Intensity</label>
              <input
                type="range"
                min="0.1"
                max="1"
                step="0.1"
                value={(mapConfig.options?.heatmapIntensity as number) ?? 0.5}
                onChange={(e) => updateOption('heatmapIntensity', Number(e.target.value))}
                className="w-32 accent-accent-link"
              />
              <span className="text-xs text-theme-text-muted w-5">
                {(mapConfig.options?.heatmapIntensity as number) ?? 0.5}
              </span>
            </div>
          </>
        )}

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

        {/* Ground snap */}
        <div className="flex items-center gap-2">
          <label className="text-xs text-theme-text-secondary w-24 shrink-0">Ground Snap</label>
          <input
            type="checkbox"
            checked={(mapConfig.options?.groundSnap as boolean) ?? false}
            onChange={(e) => updateOption('groundSnap', e.target.checked)}
          />
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
  description: '3D map visualization with heatmap overlay',
  showTypeBadge: true,
  defaultHeight: 500,

  canBlockDownstream: true,

  createDefaultConfig: () => ({
    type: 'map' as const,
    sql: DEFAULT_SQL.map,
    options: {
      showHeatmap: false,
      markerColor: '#bf360c',
      markerSize: 10,
      groundSnap: false,
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
