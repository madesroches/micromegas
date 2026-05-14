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
import {
  buildOverlay,
  defaultMappingFor,
  hexFromRgba,
  materializeRow,
  rgbaFromHex,
  type ChannelBinding,
  type OverlayMapping,
  type Shape,
} from '@/components/map/overlay'
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
// Mapping resolution
// =============================================================================
//
// `resolveMapping` synthesizes a complete `OverlayMapping` from stored options
// plus legacy `markerColor`/`markerSize` fallbacks. All back-compat lives here
// so `buildOverlay` stays agnostic of cell-config shape.

function resolveMapping(options: Record<string, unknown> | undefined): {
  shape: Shape
  mapping: OverlayMapping
} {
  const shape: Shape = (options?.shape as Shape) === 'box' ? 'box' : 'sphere'
  const stored = (options?.mapping as OverlayMapping | undefined) ?? {}

  const legacyColor = options?.markerColor as string | undefined
  const legacySize = options?.markerSize as number | undefined

  const defaults = defaultMappingFor(shape)
  const merged: OverlayMapping = { ...defaults, ...stored }

  if (!stored.color && legacyColor) {
    const parsed = rgbaFromHex(legacyColor)
    if (parsed !== null) merged.color = { scalar: parsed }
  }
  if (shape === 'sphere' && !stored.size && typeof legacySize === 'number') {
    merged.size = { scalar: legacySize }
  }

  return { shape, mapping: merged }
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
  // Key the memo on the underlying option fields, not on `resolveMapping(...)`
  // — `resolveMapping` returns a fresh object every render, and
  // `getRendererProps` spreads `options` into a new object each call. A
  // derived-object dep would re-run `buildOverlay` every render at 100K-row
  // cost.
  const overlayResult = useMemo(
    () => {
      if (!sourceTable) return null
      const { mapping } = resolveMapping(options)
      return buildOverlay(sourceTable, mapping)
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [sourceTable, options?.shape, options?.mapping, options?.markerColor, options?.markerSize]
  )
  // Narrow once; from here on, `overlay`/`constants` are paired non-null when
  // the build succeeded.
  const overlay = overlayResult?.ok ? overlayResult.overlay : null
  const constants = overlayResult?.ok ? overlayResult.constants : null
  const shape: Shape = (options?.shape as Shape) === 'box' ? 'box' : 'sphere'

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

  if (!overlay || !constants || overlay.table.numRows === 0) {
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
          constants={constants}
          shape={shape}
          selectedRowIndex={selectedRowIndex}
          onSelect={handleSelectByRowIndex}
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

/**
 * One channel-binding row in the Primitive section. The radio toggles between
 * a literal value (`scalar`) and a column reference (`column`); the inline
 * editor swaps to match.
 */
function ChannelBindingControl({
  label,
  kind,
  binding,
  fallbackScalar,
  numericRange,
  columns,
  onChange,
}: {
  label: string
  kind: 'numeric' | 'color'
  binding: ChannelBinding | undefined
  fallbackScalar: number
  numericRange?: { min: number; max: number; step: number }
  columns: string[]
  onChange: (next: ChannelBinding) => void
}) {
  const isColumn = !!binding && 'column' in binding
  const mode: 'scalar' | 'column' = isColumn ? 'column' : 'scalar'

  const setMode = (next: 'scalar' | 'column') => {
    if (next === mode) return
    if (next === 'scalar') {
      onChange({ scalar: fallbackScalar })
    } else {
      onChange({ column: columns[0] ?? '' })
    }
  }

  const scalarValue =
    binding && 'scalar' in binding ? (binding.scalar as number) : fallbackScalar

  // Split RGBA u32 into hex (#rrggbb) + alpha byte for the picker widgets.
  const colorHex = kind === 'color' ? hexFromRgba(scalarValue).slice(0, 7) : '#000000'
  const alphaByte = kind === 'color' ? scalarValue & 0xff : 0xff

  const packRgba = (hex: string, alpha: number): number => {
    const rgba = rgbaFromHex(`${hex}${alpha.toString(16).padStart(2, '0')}`)
    return rgba ?? 0
  }

  return (
    <div className="flex items-center gap-2 flex-wrap">
      <label className="text-xs text-theme-text-secondary w-24 shrink-0">{label}</label>
      <div className="flex items-center gap-3 text-xs text-theme-text-secondary">
        <label className="flex items-center gap-1 cursor-pointer">
          <input
            type="radio"
            checked={mode === 'scalar'}
            onChange={() => setMode('scalar')}
          />
          scalar
        </label>
        <label className="flex items-center gap-1 cursor-pointer">
          <input
            type="radio"
            checked={mode === 'column'}
            onChange={() => setMode('column')}
          />
          column
        </label>
      </div>

      {mode === 'scalar' && kind === 'numeric' && (
        <input
          type="number"
          value={Number.isFinite(scalarValue) ? scalarValue : 0}
          min={numericRange?.min}
          max={numericRange?.max}
          step={numericRange?.step ?? 1}
          onChange={(e) => {
            const v = Number(e.target.value)
            onChange({ scalar: Number.isFinite(v) ? v : 0 })
          }}
          className="w-24 bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary focus:outline-none focus:border-accent-link"
        />
      )}

      {mode === 'scalar' && kind === 'color' && (
        <>
          <input
            type="color"
            value={colorHex}
            onChange={(e) => onChange({ scalar: packRgba(e.target.value, alphaByte) })}
            className="w-7 h-7 rounded cursor-pointer border border-theme-border bg-transparent"
          />
          <label className="text-xs text-theme-text-muted">alpha</label>
          <input
            type="range"
            min="0"
            max="255"
            value={alphaByte}
            onChange={(e) =>
              onChange({ scalar: packRgba(colorHex, Number(e.target.value)) })
            }
            className="w-24 accent-accent-link"
          />
          <span className="text-xs text-theme-text-muted w-7">{alphaByte}</span>
        </>
      )}

      {mode === 'column' && (
        <select
          value={isColumn ? (binding as { column: string }).column : ''}
          onChange={(e) => onChange({ column: e.target.value })}
          className="flex-1 bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary focus:outline-none focus:border-accent-link"
        >
          {columns.length === 0 && (
            <option value="">No columns available — run the query</option>
          )}
          {columns.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      )}
    </div>
  )
}

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

  // Read the persisted `shape` + `mapping`, layered with the per-shape
  // defaults so empty channels resolve to the same scalars buildOverlay would
  // use. Legacy `markerColor`/`markerSize` aren't re-read here on purpose —
  // those exist only for the runtime back-compat path; the editor writes
  // the new `mapping` shape from first touch.
  const shape: Shape =
    (mapConfig.options?.shape as Shape) === 'box' ? 'box' : 'sphere'
  const storedMapping =
    (mapConfig.options?.mapping as OverlayMapping | undefined) ?? {}
  const mappingDefaults = defaultMappingFor(shape)

  const channelBinding = (channel: keyof OverlayMapping): ChannelBinding | undefined => {
    const v = (storedMapping[channel] ?? mappingDefaults[channel]) as
      | ChannelBinding
      | undefined
    return v
  }

  const updateMappingChannel = useCallback(
    (channel: keyof OverlayMapping, next: ChannelBinding) => {
      const prev = (mapConfig.options?.mapping as OverlayMapping | undefined) ?? {}
      const nextMapping = { ...prev, [channel]: next }
      onChange({
        ...mapConfig,
        options: { ...mapConfig.options, mapping: nextMapping },
      })
    },
    [mapConfig, onChange]
  )

  const updateShape = useCallback(
    (next: Shape) => {
      onChange({
        ...mapConfig,
        options: { ...mapConfig.options, shape: next },
      })
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
      </div>

      {/* Primitive */}
      <div className="space-y-3">
        <label className="block text-xs font-medium text-theme-text-secondary uppercase">
          Primitive
        </label>

        <div className="flex items-center gap-2">
          <label className="text-xs text-theme-text-secondary w-24 shrink-0">Shape</label>
          <select
            value={shape}
            onChange={(e) => updateShape(e.target.value as Shape)}
            className="bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary focus:outline-none focus:border-accent-link"
          >
            <option value="sphere">Sphere</option>
            <option value="box">Box</option>
          </select>
        </div>

        {shape === 'sphere' && (
          <>
            <ChannelBindingControl
              label="Size"
              kind="numeric"
              binding={channelBinding('size')}
              fallbackScalar={10}
              numericRange={{ min: 0, max: 10000, step: 1 }}
              columns={availableColumns ?? []}
              onChange={(b) => updateMappingChannel('size', b)}
            />
            <ChannelBindingControl
              label="Color"
              kind="color"
              binding={channelBinding('color')}
              fallbackScalar={0xbf360cff}
              columns={availableColumns ?? []}
              onChange={(b) => updateMappingChannel('color', b)}
            />
          </>
        )}

        {shape === 'box' && (
          <>
            <ChannelBindingControl
              label="Scale X"
              kind="numeric"
              binding={channelBinding('scaleX')}
              fallbackScalar={100}
              numericRange={{ min: 0, max: 100000, step: 1 }}
              columns={availableColumns ?? []}
              onChange={(b) => updateMappingChannel('scaleX', b)}
            />
            <ChannelBindingControl
              label="Scale Y"
              kind="numeric"
              binding={channelBinding('scaleY')}
              fallbackScalar={100}
              numericRange={{ min: 0, max: 100000, step: 1 }}
              columns={availableColumns ?? []}
              onChange={(b) => updateMappingChannel('scaleY', b)}
            />
            <ChannelBindingControl
              label="Scale Z"
              kind="numeric"
              binding={channelBinding('scaleZ')}
              fallbackScalar={100}
              numericRange={{ min: 0, max: 100000, step: 1 }}
              columns={availableColumns ?? []}
              onChange={(b) => updateMappingChannel('scaleZ', b)}
            />
            <ChannelBindingControl
              label="Color"
              kind="color"
              binding={channelBinding('color')}
              fallbackScalar={0xbf360cff}
              columns={availableColumns ?? []}
              onChange={(b) => updateMappingChannel('color', b)}
            />
          </>
        )}
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
      shape: 'sphere' as const,
      mapping: {
        size: { scalar: 10 },
        color: { scalar: 0xbf360cff },
      },
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
