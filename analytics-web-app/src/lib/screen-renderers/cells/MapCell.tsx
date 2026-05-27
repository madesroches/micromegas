import { Suspense, use, useState, useCallback, useMemo, useEffect, useRef, startTransition } from 'react'
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
import { MapHoverTooltip } from '@/components/map/MapHoverTooltip'
import {
  buildOverlay,
  columnTypeMap,
  defaultMappingFor,
  hexFromRgba,
  resolveMappingScalars,
  resolveOverlayConstants,
  rgbaFromHex,
  rowValues,
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
  const selectedFilename = selectedRaw
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
// `resolveMapping` synthesizes a complete `OverlayMapping` from stored options,
// layering per-shape defaults under the stored channels and dropping bindings
// that don't apply to the active shape — so `buildOverlay` stays agnostic of
// cell-config shape.

function resolveMapping(options: Record<string, unknown> | undefined): {
  shape: Shape
  mapping: OverlayMapping
} {
  const shape: Shape = (options?.shape as Shape) === 'box' ? 'box' : 'sphere'
  const stored = (options?.mapping as OverlayMapping | undefined) ?? {}

  // Drop channels that don't apply to the active shape *before* the merge.
  // The editor's updateShape callback already strips these on toggle, but
  // configs persisted from older versions or edited externally can carry
  // stale bindings — letting them flow into buildOverlay surfaces a
  // confusing "Column 'foo' for channel 'size' not found" when the user's
  // actual issue is that the binding doesn't apply to the current shape.
  // Only assign defined keys so a missing channel doesn't shadow the default
  // when spread.
  const filtered: OverlayMapping = {}
  if (stored.x !== undefined) filtered.x = stored.x
  if (stored.y !== undefined) filtered.y = stored.y
  if (stored.z !== undefined) filtered.z = stored.z
  if (stored.color !== undefined) filtered.color = stored.color
  if (shape === 'sphere') {
    if (stored.size !== undefined) filtered.size = stored.size
  } else {
    if (stored.scaleX !== undefined) filtered.scaleX = stored.scaleX
    if (stored.scaleY !== undefined) filtered.scaleY = stored.scaleY
    if (stored.scaleZ !== undefined) filtered.scaleZ = stored.scaleZ
  }

  const defaults = defaultMappingFor(shape)
  const merged: OverlayMapping = { ...defaults, ...filtered }

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
  // Two-layer memo so editor scrubbing stays cheap.
  //
  // `buildOverlay` walks every row to bake position/scale/color buffers — at
  // 100K rows this is the expensive path. Keying its memo on the *structural*
  // shape of the mapping (which channels are column-bound and which column
  // names they reference) lets scalar-only edits — alpha slider, color
  // picker, size scrub — skip the row walk and reuse the prior overlay. The
  // changed scalars instead flow through `resolveOverlayConstants` into a
  // cheap `constants` object the renderer reads at draw time.
  const mappingObj = options?.mapping as OverlayMapping | undefined
  const bindingColumn = (b: ChannelBinding<unknown> | undefined): string | null =>
    b && 'column' in b ? b.column : null
  // Position channels accept either `{column}` or `{scalar: 'colname'}` — both
  // resolve to a column-name string. Mirror resolvePositionColumn so the dep
  // captures column-name changes in either form.
  const positionColumn = (
    b: ChannelBinding<string> | undefined,
    fallback: string,
  ): string => (b ? ('column' in b ? b.column : b.scalar) : fallback)
  const xCol = positionColumn(mappingObj?.x, 'x')
  const yCol = positionColumn(mappingObj?.y, 'y')
  const zCol = positionColumn(mappingObj?.z, 'z')
  const sizeCol = bindingColumn(mappingObj?.size)
  const scaleXCol = bindingColumn(mappingObj?.scaleX)
  const scaleYCol = bindingColumn(mappingObj?.scaleY)
  const scaleZCol = bindingColumn(mappingObj?.scaleZ)
  const colorCol = bindingColumn(mappingObj?.color)

  const overlayBuildResult = useMemo(
    () => {
      if (!sourceTable) return null
      const { mapping } = resolveMapping(options)
      return buildOverlay(sourceTable, mapping)
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [
      sourceTable,
      options?.shape,
      xCol,
      yCol,
      zCol,
      sizeCol,
      scaleXCol,
      scaleYCol,
      scaleZCol,
      colorCol,
    ],
  )
  // Scalar fallbacks resolve on every render (cheap — just reads from
  // mapping). When `buildOverlay` skipped allocating `overlay.colorsRGBA`
  // because color is scalar, the renderer reads `constants.color` from here
  // instead.
  //
  // `resolveMappingScalars` expands any `$macro` strings stored in scalar
  // bindings against the notebook macro context. Decoupled from the heavy
  // buildOverlay memo: a macro that drives a scalar (e.g. `$mySize`) changes
  // the scalar value without changing which column the channel reads from,
  // so we re-resolve constants but skip the row walk.
  const resolvedMappingResult = useMemo(
    () => {
      const { mapping } = resolveMapping(options)
      return resolveMappingScalars(mapping, {
        variables,
        timeRange,
        cellResults,
        cellSelections,
      })
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [
      options?.shape,
      options?.mapping,
      variables,
      timeRange.begin,
      timeRange.end,
      cellResults,
      cellSelections,
    ],
  )
  const constants = useMemo(
    () =>
      resolvedMappingResult.ok
        ? resolveOverlayConstants(resolvedMappingResult.mapping)
        : resolveOverlayConstants(undefined),
    [resolvedMappingResult],
  )
  // Surface a macro-resolution failure as if it were a build failure — same
  // error rendering surface, so the user sees one consistent place for
  // mapping problems whether they're column-, scalar-, or macro-driven.
  const overlayResult = resolvedMappingResult.ok ? overlayBuildResult : resolvedMappingResult
  const overlay = overlayResult?.ok ? overlayResult.overlay : null
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
  // notebooks keep working across base-path changes.
  const mapFilename = options?.mapUrl as string | undefined
  const basePath = getConfig().basePath
  const mapBlobUrl = useMemo(
    () => resolveMapBlobUrl(mapFilename, basePath),
    [mapFilename, basePath]
  )
  const detailTemplate =
    (options?.detailTemplate as string | undefined) ?? DEFAULT_MAP_DETAIL_TEMPLATE
  // Hover tooltip is opt-out: absent (legacy configs) or `true` → shown.
  const showHoverTooltip = (options?.showHoverTooltip as boolean | undefined) !== false

  const handleSelectByRowIndex = useCallback(
    (rowIndex: number | null) => {
      setSelectedRowIndex(rowIndex)
      if (rowIndex === null || !overlay) {
        onSelectionChangeRef.current?.(null)
      } else {
        onSelectionChangeRef.current?.(rowValues(overlay.table, rowIndex))
      }
    },
    [overlay]
  )

  const selectedRow = useMemo(
    () =>
      selectedRowIndex !== null && overlay
        ? rowValues(overlay.table, selectedRowIndex)
        : null,
    [selectedRowIndex, overlay]
  )

  // Schema-derived type map for the detail template's bare `$col` macros —
  // depends only on the table schema, so it's memoized off `overlay.table`
  // rather than the selected row index.
  const columnTypes = useMemo(
    () => (overlay ? columnTypeMap(overlay.table) : null),
    [overlay]
  )

  // Transient hover preview: row index + cursor position, lifted from the mesh
  // via MapViewer's onHover. Position drives the tooltip; the row index drives
  // content.
  const [hover, setHover] = useState<{ rowIndex: number; x: number; y: number } | null>(null)
  const handleHover = useCallback(
    (rowIndex: number | null, x: number, y: number) => {
      setHover(rowIndex === null ? null : { rowIndex, x, y })
    },
    []
  )

  // Clear stale hover on overlay swap (render-phase). The mesh only calls
  // onHover from pointer events; when `overlay` changes with no pointer event,
  // MapInstancedMarkers clears its own hoveredRowIndex but never calls
  // onHover(null), so this lifted `hover` would keep the old rowIndex and the
  // memo below would derive rowValues against the new table (no bounds check).
  // Mirror overlayForSelection: reset hover during render before the derive.
  const [hoverOverlay, setHoverOverlay] = useState(overlay)
  if (hoverOverlay !== overlay) {
    setHoverOverlay(overlay)
    if (hover !== null) setHover(null)
  }

  // Memoized on rowIndex (not x/y): cursor movement repositions the tooltip
  // without re-deriving the row or re-evaluating the template.
  const hoveredRow = useMemo(
    () => (hover && overlay ? rowValues(overlay.table, hover.rowIndex) : null),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [hover?.rowIndex, overlay]
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
          constants={constants}
          shape={shape}
          selectedRowIndex={selectedRowIndex}
          onSelect={handleSelectByRowIndex}
          onHover={handleHover}
          resetViewTrigger={resetViewTrigger}
        />

        {selectedRow && columnTypes && (
          <EventDetailPanel
            row={selectedRow}
            columnTypes={columnTypes}
            template={detailTemplate}
            variables={variables}
            timeRange={timeRange}
            cellResults={cellResults}
            cellSelections={cellSelections}
            onClose={() => handleSelectByRowIndex(null)}
          />
        )}

        {/* Transient cursor preview, same content as the docked panel. Shown
            for any hovered marker (including the selected one) when the editor
            option is enabled. The detailTemplate.trim() guard implements
            "blank template → highlight only, no preview". */}
        {showHoverTooltip && hover && hoveredRow && columnTypes && detailTemplate.trim() && (
          <MapHoverTooltip
            x={hover.x}
            y={hover.y}
            row={hoveredRow}
            columnTypes={columnTypes}
            template={detailTemplate}
            variables={variables}
            timeRange={timeRange}
            cellResults={cellResults}
            cellSelections={cellSelections}
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
 *
 * Scalar input is a plain text field — whatever the user types is stored as
 * a string and passed through macro substitution at render time, then parsed
 * to a number (for numeric channels) or `#rrggbb[aa]` (for color). This is
 * what lets `$mySize` or `$cell.selected.radius` drive the value. Legacy
 * scalars stored as raw numbers/RGBA-u32 still load — they're rendered in
 * their canonical string form (`"10"` or `"#bf360cff"`) on first edit.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function ChannelBindingControl({
  label,
  kind,
  binding,
  fallbackScalar,
  columns,
  onChange,
}: {
  label: string
  kind: 'numeric' | 'color'
  binding: ChannelBinding | undefined
  fallbackScalar: number
  columns: string[]
  onChange: (next: ChannelBinding) => void
}) {
  const isColumn = !!binding && 'column' in binding
  const mode: 'scalar' | 'column' = isColumn ? 'column' : 'scalar'

  const formatScalar = (value: number): string =>
    kind === 'color' ? hexFromRgba(value) : String(value)

  const noColumns = columns.length === 0
  const setMode = (next: 'scalar' | 'column') => {
    if (next === mode) return
    if (next === 'scalar') {
      onChange({ scalar: formatScalar(fallbackScalar) })
    } else {
      // Guard: writing `{column: ''}` produces an unresolvable binding that
      // fails buildOverlay with `Column '' not found`. Force scalar mode
      // until a query result is available.
      if (noColumns) return
      onChange({ column: columns[0] })
    }
  }

  // Render legacy numeric scalars (number / RGBA u32) in their string form so
  // the text input can edit them; future edits roundtrip as strings.
  const scalarString: string = (() => {
    if (!binding || !('scalar' in binding)) return formatScalar(fallbackScalar)
    if (typeof binding.scalar === 'string') return binding.scalar
    if (typeof binding.scalar === 'number') return formatScalar(binding.scalar)
    return formatScalar(fallbackScalar)
  })()

  // Decouple typing from the model. The text input owns an in-flight `draft`
  // and only commits to `onChange` on blur or Enter. Without this, every
  // keystroke would round-trip through `updateMappingChannel` (which wraps
  // the parent update in `startTransition`); the deferred re-render then
  // resyncs `value` mid-typing and the browser resets the cursor position.
  //
  // The ref-guarded sync block adopts the prop value when it changes from
  // outside this control — mode switch, color-picker pick, saved-config
  // reload — so external updates win over uncommitted typing.
  const [draft, setDraft] = useState(scalarString)
  const lastSyncedScalarRef = useRef(scalarString)
  if (lastSyncedScalarRef.current !== scalarString) {
    lastSyncedScalarRef.current = scalarString
    setDraft(scalarString)
  }
  // Escape resets the draft and then blurs the input — but React batches the
  // `setDraft` so the immediately-following `onBlur` reads the stale typed
  // value from the closure and would commit it. This ref tells commitDraft
  // to skip the next call, so Escape reliably cancels.
  const skipNextCommitRef = useRef(false)
  const commitDraft = () => {
    if (skipNextCommitRef.current) {
      skipNextCommitRef.current = false
      return
    }
    if (draft !== scalarString) {
      onChange({ scalar: draft })
    }
  }

  const placeholder =
    kind === 'color' ? '#bf360cff or $myColor' : 'e.g. 10 or $mySize'

  // For the color channel, parse the current text into an RGBA u32 so the
  // picker swatch reflects literal hex values. Macros and invalid text fall
  // back to the channel's default — clicking the picker then writes a fresh
  // `#rrggbbff`, replacing whatever non-hex string was there. Read from the
  // committed `scalarString`, not the in-flight `draft`, so the swatch
  // doesn't strobe through partially-typed hex codes.
  const parsedRgba =
    kind === 'color' ? rgbaFromHex(scalarString) ?? fallbackScalar : 0
  const colorPickerHex = hexFromRgba(parsedRgba).slice(0, 7)
  const currentAlpha = parsedRgba & 0xff

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
        <label
          className={`flex items-center gap-1 ${noColumns ? 'cursor-not-allowed opacity-50' : 'cursor-pointer'}`}
          title={noColumns ? 'Run the query to populate available columns' : undefined}
        >
          <input
            type="radio"
            checked={mode === 'column'}
            disabled={noColumns}
            onChange={() => setMode('column')}
          />
          column
        </label>
      </div>

      {mode === 'scalar' && (
        <input
          type="text"
          value={draft}
          placeholder={placeholder}
          spellCheck={false}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commitDraft}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              // Blur triggers commitDraft via onBlur — keeps the commit path
              // single-sourced so Enter and click-away behave identically.
              e.currentTarget.blur()
            } else if (e.key === 'Escape') {
              // Cancel the in-flight edit, restoring the committed value. The
              // flag is required because React batches `setDraft` — without
              // it, the synchronous `onBlur` would still see the stale typed
              // draft from this render's closure and commit it.
              skipNextCommitRef.current = true
              setDraft(scalarString)
              e.currentTarget.blur()
            }
          }}
          className="flex-1 min-w-[10rem] bg-app-card border border-theme-border rounded px-2 py-1 text-sm font-mono text-theme-text-primary focus:outline-none focus:border-accent-link"
        />
      )}

      {mode === 'scalar' && kind === 'color' && (
        <input
          type="color"
          value={colorPickerHex}
          // HTML <input type="color"> doesn't carry alpha, so preserve the
          // current text's alpha byte (or 0xff for non-hex inputs) when the
          // picker fires. Picking always replaces the text with a fresh hex,
          // so a macro typed into the field will be overwritten — that's the
          // intended affordance for "switch back to a literal color".
          onChange={(e) => {
            const alphaHex = currentAlpha.toString(16).padStart(2, '0')
            onChange({ scalar: `${e.target.value}${alphaHex}` })
          }}
          aria-label={`${label} color picker`}
          title="Pick a color (replaces the text value)"
          className="w-7 h-7 rounded cursor-pointer border border-theme-border bg-transparent shrink-0"
        />
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
  // use.
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
      // Wrap in startTransition so React treats the update as non-urgent.
      // Color/alpha sliders fire onChange on every pointer move (~60–120 Hz
      // in Firefox); each event would otherwise trigger a full re-render
      // tree through screenConfig. Marking the update as a transition lets
      // React drop stale work mid-render when newer events arrive — without
      // it, the cumulative commit depth hits React's 50-update ceiling and
      // surfaces as "Maximum update depth exceeded".
      startTransition(() => {
        const prev = (mapConfig.options?.mapping as OverlayMapping | undefined) ?? {}
        const nextMapping = { ...prev, [channel]: next }
        onChange({
          ...mapConfig,
          options: { ...mapConfig.options, mapping: nextMapping },
        })
      })
    },
    [mapConfig, onChange]
  )

  const updateShape = useCallback(
    (next: Shape) => {
      // Drop channels that don't apply to the new shape. Without this, a
      // column-bound `size` left over from sphere mode (or `scaleX/Y/Z` from
      // box mode) would still be validated by buildOverlay against the new
      // query result — surfacing a confusing "Column 'foo' for channel
      // 'size' not found" when the user's actual issue is just that the
      // binding is stale.
      const prev = (mapConfig.options?.mapping as OverlayMapping | undefined) ?? {}
      const nextMapping: OverlayMapping = {}
      if (prev.x !== undefined) nextMapping.x = prev.x
      if (prev.y !== undefined) nextMapping.y = prev.y
      if (prev.z !== undefined) nextMapping.z = prev.z
      if (prev.color !== undefined) nextMapping.color = prev.color
      if (next === 'sphere') {
        if (prev.size !== undefined) nextMapping.size = prev.size
      } else {
        if (prev.scaleX !== undefined) nextMapping.scaleX = prev.scaleX
        if (prev.scaleY !== undefined) nextMapping.scaleY = prev.scaleY
        if (prev.scaleZ !== undefined) nextMapping.scaleZ = prev.scaleZ
      }
      onChange({
        ...mapConfig,
        options: { ...mapConfig.options, shape: next, mapping: nextMapping },
      })
    },
    [mapConfig, onChange]
  )

  const validationErrors = useMemo(() => {
    return validateMacros(mapConfig.sql, variables, cellResults, cellSelections).errors
  }, [mapConfig.sql, variables, cellResults, cellSelections])

  const detailTemplate =
    (mapConfig.options?.detailTemplate as string | undefined) ?? DEFAULT_MAP_DETAIL_TEMPLATE
  const showHoverTooltip =
    (mapConfig.options?.showHoverTooltip as boolean | undefined) !== false

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
          minHeight="240px"
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
              columns={availableColumns ?? []}
              onChange={(b) => updateMappingChannel('scaleX', b)}
            />
            <ChannelBindingControl
              label="Scale Y"
              kind="numeric"
              binding={channelBinding('scaleY')}
              fallbackScalar={100}
              columns={availableColumns ?? []}
              onChange={(b) => updateMappingChannel('scaleY', b)}
            />
            <ChannelBindingControl
              label="Scale Z"
              kind="numeric"
              binding={channelBinding('scaleZ')}
              fallbackScalar={100}
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
        <label className="flex items-center gap-2 mt-2 text-xs text-theme-text-secondary cursor-pointer select-none">
          <input
            type="checkbox"
            checked={showHoverTooltip}
            onChange={(e) => updateOption('showHoverTooltip', e.target.checked)}
            className="accent-accent-link"
          />
          Show as a tooltip when hovering a marker
        </label>
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
      showHoverTooltip: true,
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
