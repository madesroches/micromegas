import { Suspense, use, useState, useCallback, useMemo, useEffect, useRef } from 'react'
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
import {
  substituteMacros,
  validateMacros,
  DEFAULT_SQL,
  DEFAULT_MAP_DETAIL_TEMPLATE,
} from '../notebook-utils'
import { MapViewer } from '@/components/map/MapViewer'
import { EventDetailPanel } from '@/components/map/EventDetailPanel'
import { buildOverlay, materializeRow } from '@/components/map/overlay'
import { ErrorBoundary } from '@/components/ErrorBoundary'
import { useGLTF } from '@react-three/drei'
import {
  type MapCatalogEntry,
  fetchMapCatalog,
  formatMapName,
  normalizeMapFilename,
  resolveMapBlobUrl,
} from '@/lib/maps-catalog'
import { getConfig } from '@/lib/config'
import { Map as MapIcon } from 'lucide-react'

// =============================================================================
// Map Catalog (loaded from /api/maps/catalog)
// =============================================================================
//
// `useMapCatalog` reads the cached catalog promise via React 19's `use()`,
// so callers see a resolved `MapCatalogEntry[]` synchronously — there is
// no in-component "loading" state to thread through render code. Any
// caller must be inside a `<Suspense>` boundary that handles the first
// render's suspension; the editor's dropdown wraps itself in one below.

function useMapCatalog(): MapCatalogEntry[] {
  const basePath = getConfig().basePath
  return use(fetchMapCatalog(basePath))
}

function MapDropdownLoading() {
  return (
    <select
      className="flex-1 bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-muted"
      value=""
      disabled
    >
      <option value="">Loading maps…</option>
    </select>
  )
}

function MapDropdown({
  selectedRaw,
  onChange,
}: {
  selectedRaw: string | undefined
  onChange: (value: string | undefined) => void
}) {
  const mapCatalog = useMapCatalog()
  const selectedFilename = normalizeMapFilename(selectedRaw)
  // When the saved mapUrl doesn't match any catalog entry, a controlled
  // `<select value={X}>` with no matching `<option>` is browser-defined:
  // most show the first option visually, and clicking that already-displayed
  // option fires no change. So we synthesize a placeholder `<option>` for
  // the stale value — the catalog is guaranteed loaded by the time this
  // renders (Suspense above), so a working map never flashes as missing.
  const isInCatalog =
    !!selectedFilename && mapCatalog.some((entry) => entry.file === selectedFilename)
  return (
    <select
      className="flex-1 bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary focus:outline-none focus:border-accent-link"
      value={selectedFilename ?? ''}
      onChange={(e) => onChange(e.target.value || undefined)}
    >
      <option value="" disabled>
        Select a map…
      </option>
      {selectedFilename && !isInCatalog && (
        <option value={selectedFilename}>
          {formatMapName(selectedFilename)} (not in catalog)
        </option>
      )}
      {mapCatalog.map((entry) => (
        <option key={entry.file} value={entry.file}>
          {formatMapName(entry.file)}
        </option>
      ))}
    </select>
  )
}

// =============================================================================
// Renderer Component
// =============================================================================

export function MapCell({
  data,
  status,
  options,
  variables,
  timeRange,
  cellResults,
  cellSelections,
  onSelectionChange,
}: CellRendererProps) {
  const sourceTable = data[0]
  const overlayResult = useMemo(
    () => (sourceTable ? buildOverlay(sourceTable) : null),
    [sourceTable]
  )
  // Narrow once; `overlay` is `Overlay | null` from here on.
  const overlay = overlayResult?.ok ? overlayResult.overlay : null

  const [selectedRowIndex, setSelectedRowIndex] = useState<number | null>(null)
  const [resetViewTrigger, setResetViewTrigger] = useState(0)

  // Clear selection synchronously when the overlay changes, before
  // `selectedRow` is derived for the new overlay — otherwise a stale row
  // index materializes against the new table for one render and the panel
  // briefly shows the wrong row.
  const [overlayForSelection, setOverlayForSelection] = useState(overlay)
  if (overlayForSelection !== overlay) {
    setOverlayForSelection(overlay)
    setSelectedRowIndex(null)
  }

  // Stable ref for onSelectionChange to avoid infinite re-render loops:
  // the callback is an inline arrow in NotebookRenderer, so including it
  // in effect deps would fire on every render.
  const onSelectionChangeRef = useRef(onSelectionChange)
  onSelectionChangeRef.current = onSelectionChange

  // Publish the clear to upstream cells after commit — calling
  // onSelectionChange during render would trigger React's "Cannot update a
  // component while rendering a different component" warning, because it
  // resolves to a parent setState through updateCellSelection.
  useEffect(() => {
    onSelectionChangeRef.current?.(null)
  }, [overlay])

  // Read visual options with defaults. `mapUrl` is stored as the bare
  // filename — the renderer composes the blob URL at render time so saved
  // notebooks keep working across base-path changes and the legacy
  // `/maps/...` → `${basePath}/api/maps/blob/...` transition.
  const mapFilename = options?.mapUrl as string | undefined
  const basePath = getConfig().basePath
  const mapBlobUrl = useMemo(
    () => resolveMapBlobUrl(mapFilename, basePath),
    [mapFilename, basePath]
  )
  const markerColor = (options?.markerColor as string) ?? '#bf360c'
  const markerSize = (options?.markerSize as number) ?? 10
  const detailTemplate =
    (options?.detailTemplate as string | undefined) ?? DEFAULT_MAP_DETAIL_TEMPLATE

  const handleSelectByRowIndex = useCallback(
    (rowIndex: number | null) => {
      setSelectedRowIndex(rowIndex)
      if (rowIndex === null || !overlay) {
        onSelectionChangeRef.current?.(null)
      } else {
        onSelectionChangeRef.current?.(materializeRow(overlay.table, rowIndex))
      }
    },
    [overlay]
  )

  const selectedRow = useMemo(
    () =>
      selectedRowIndex !== null && overlay
        ? materializeRow(overlay.table, selectedRowIndex)
        : null,
    [selectedRowIndex, overlay]
  )

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

  if (overlayResult && !overlayResult.ok) {
    return (
      <div className="flex items-center justify-center h-full text-theme-text-muted text-sm whitespace-pre-wrap text-center px-4">
        {overlayResult.error}
      </div>
    )
  }

  if (!overlay || overlay.table.numRows === 0) {
    return (
      <div className="flex items-center justify-center h-full text-theme-text-muted text-sm">
        No spatial data available. Query must return columns: x, y, z
      </div>
    )
  }

  if (!mapBlobUrl) {
    return (
      <div className="flex items-center justify-center h-full text-theme-text-muted text-sm">
        No map selected. Open the editor and pick a map from the dropdown.
      </div>
    )
  }

  return (
    <div ref={containerRef} className="relative w-full h-full overflow-hidden">
      {/* Keyed by URL: a new map gets a fresh boundary, so picking a working
          map after a failed load clears the error automatically. The fallback
          stays scoped to the cell so a 404 (or any GLB load failure) doesn't
          bubble up to the screen-level ErrorBoundary and replace the whole
          page with "Something went wrong". */}
      <ErrorBoundary
        key={mapBlobUrl}
        fallback={(error, reset) => (
          <div className="flex flex-col items-center justify-center h-full p-4 text-center">
            <div className="text-red-400 text-sm font-medium mb-2">Could not load map</div>
            {mapFilename && (
              <div className="text-xs text-theme-text-muted font-mono break-all mb-2">
                {mapFilename}
              </div>
            )}
            <div className="text-xs text-theme-text-muted mb-3 max-w-md break-words">
              {error.message}
            </div>
            <button
              className="px-3 py-1 text-xs bg-app-card border border-theme-border rounded hover:border-accent-link"
              onClick={() => {
                // Clear drei's cached failed promise — otherwise a retry on
                // the same URL re-throws the same error without refetching.
                useGLTF.clear(mapBlobUrl)
                reset()
              }}
            >
              Try again
            </button>
          </div>
        )}
      >
        <MapViewer
          mapUrl={mapBlobUrl}
          overlay={overlay}
          selectedRowIndex={selectedRowIndex}
          onSelect={handleSelectByRowIndex}
          markerColor={markerColor}
          markerSize={markerSize}
          resetViewTrigger={resetViewTrigger}
        />

        {selectedRow && (
          <EventDetailPanel
            row={selectedRow}
            template={detailTemplate}
            variables={variables}
            timeRange={timeRange}
            cellResults={cellResults}
            cellSelections={cellSelections}
            onClose={() => handleSelectByRowIndex(null)}
          />
        )}
      </ErrorBoundary>
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
  availableColumns,
  cellResults,
  cellSelections,
}: CellEditorProps) {
  const mapConfig = config as QueryCellConfig

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

  const detailTemplate =
    (mapConfig.options?.detailTemplate as string | undefined) ?? DEFAULT_MAP_DETAIL_TEMPLATE

  // Empty-string placeholders for every column from the most recent result
  // so the editor doesn't flag `$columnFromQuery` as "Unknown variable" at
  // edit time. validateMacros only checks presence.
  const templateValidationErrors = useMemo(() => {
    const syntheticColumnVars: Record<string, string> = {}
    for (const name of availableColumns ?? []) {
      syntheticColumnVars[name] = ''
    }
    const mergedVars = { ...variables, ...syntheticColumnVars }
    return validateMacros(detailTemplate, mergedVars, cellResults, cellSelections).errors
  }, [detailTemplate, availableColumns, variables, cellResults, cellSelections])

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
            <Suspense fallback={<MapDropdownLoading />}>
              <MapDropdown
                selectedRaw={mapConfig.options?.mapUrl as string | undefined}
                onChange={(value) => updateOption('mapUrl', value)}
              />
            </Suspense>
          </div>
          <div className="text-xs text-theme-text-muted ml-[calc(6rem+0.5rem)]">
            Maps are loaded from the server's object store (
            <code className="text-theme-text-secondary">MICROMEGAS_MAPS_OBJECT_STORE_URI</code>).
            Drop <code className="text-theme-text-secondary">.glb</code> files at that prefix to
            make them appear here.
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

      {/* Detail template */}
      <div>
        <label className="block text-xs font-medium text-theme-text-secondary uppercase mb-1.5">
          Detail Template (Markdown)
        </label>
        <SyntaxEditor
          value={detailTemplate}
          onChange={(value) => updateOption('detailTemplate', value)}
          language="markdown"
          placeholder="### Event\n\n**Location:** ($x, $y, $z)"
          minHeight="160px"
        />
        <div className="text-xs text-theme-text-muted mt-1">
          Rendered when a marker is selected. References query columns as
          <code className="text-theme-text-secondary mx-1">$colname</code>
          and notebook variables as <code className="text-theme-text-secondary">$var</code>.
        </div>
      </div>
      {templateValidationErrors.length > 0 && (
        <div className="text-red-400 text-sm space-y-1">
          {templateValidationErrors.map((err, i) => (
            <div key={i}>Warning: {err}</div>
          ))}
        </div>
      )}

      <AvailableVariablesPanel
        variables={variables}
        timeRange={timeRange}
        cellResults={cellResults}
        cellSelections={cellSelections}
        localRowColumns={availableColumns}
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

  defaultSelectionMode: 'single',

  createDefaultConfig: () => ({
    type: 'map' as const,
    sql: DEFAULT_SQL.map,
    options: {
      markerColor: '#bf360c',
      markerSize: 10,
      detailTemplate: DEFAULT_MAP_DETAIL_TEMPLATE,
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
