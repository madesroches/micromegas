import { useState, useCallback, useMemo } from 'react'
import type { DataType } from 'apache-arrow'
import { ChevronDown, ChevronRight, Plus, X, AlertTriangle } from 'lucide-react'
import { type ColumnOverride, validateFormatMacros } from '@/lib/screen-renderers/table-utils'
import { isHistogramStructType } from '@/lib/arrow-utils'
import { COLORMAP_NAMES, resolveHistogramBarColor, buildColormapPreviewGradient } from '@/lib/histogram-colors'

interface OverrideEditorProps {
  overrides: ColumnOverride[]
  availableColumns: string[]
  /** Column name -> DataType, for the "Render as" toggle (only shown for a
   *  histogram-typed column, or a card already saved with kind: 'histogram'). */
  availableColumnTypes?: Record<string, DataType>
  /** Variable names available for macro substitution (e.g., from notebook cells) */
  availableVariables?: string[]
  /** Cell names that support $cell.selected.column syntax */
  cellSelectionNames?: string[]
  onChange: (overrides: ColumnOverride[]) => void
}

/** `kind` defaults to 'markdown' when unset — every override saved before
 *  the histogram feature existed already behaves this way. */
function effectiveKind(override: ColumnOverride): 'markdown' | 'histogram' {
  return override.kind === 'histogram' ? 'histogram' : 'markdown'
}

const DEFAULT_FORMAT_TEMPLATE = (column: string) => `[Link](/path?id=$row.${column})`

// =============================================================================
// Histogram color swatch picker (Design §6's Editor UI)
// =============================================================================

/** Deterministic sample shape for the live bar preview — not real data, just
 *  enough bins to show the same normalized-height math the real cell uses. */
const PREVIEW_BINS = [3, 7, 14, 22, 30, 26, 18, 10, 5, 2]
const PREVIEW_TRACK_WIDTH = 120
const PREVIEW_TRACK_HEIGHT = 28

function isColormapName(value: string): boolean {
  return (COLORMAP_NAMES as readonly string[]).includes(value)
}

function HistogramColorPreview({ color }: { color?: string }) {
  const max = Math.max(...PREVIEW_BINS)
  const barWidth = PREVIEW_TRACK_WIDTH / PREVIEW_BINS.length
  return (
    <svg
      width={PREVIEW_TRACK_WIDTH}
      height={PREVIEW_TRACK_HEIGHT}
      viewBox={`0 0 ${PREVIEW_TRACK_WIDTH} ${PREVIEW_TRACK_HEIGHT}`}
      preserveAspectRatio="none"
      className="mt-2"
    >
      {PREVIEW_BINS.map((v, i) => {
        const t = v / max
        const height = Math.max(2, t * PREVIEW_TRACK_HEIGHT)
        return (
          <rect
            key={i}
            x={i * barWidth}
            y={PREVIEW_TRACK_HEIGHT - height}
            width={barWidth}
            height={height}
            fill={resolveHistogramBarColor(color, t)}
          />
        )
      })}
    </svg>
  )
}

interface HistogramColorPickerProps {
  value?: string
  onChange: (value: string | undefined) => void
}

/**
 * A swatch picker for `ColumnOverride.histogramColor` — no free-text field,
 * no colormap names to type or read (Design §6, Trade-offs: "don't tell,
 * show"). A leading Default swatch clears the field; six colormap swatches
 * (each rendered as its own mini-gradient via `buildColormapPreviewGradient`)
 * set a recognized name; one custom-color swatch (backed by a native
 * `<input type="color">`) sets a literal hex. Whichever matches the current
 * value gets a highlighted ring.
 */
function HistogramColorPicker({ value, onChange }: HistogramColorPickerProps) {
  const isCustom = value != null && !isColormapName(value)
  const customSwatchColor = isCustom ? value : '#bf360c'
  const swatchBase = 'w-6 h-6 rounded-sm border-2 transition-colors'
  const ringClass = 'border-accent-link'
  const idleClass = 'border-theme-border'

  return (
    <div>
      <label className="block text-xs font-medium text-theme-text-secondary mb-1">Bar Color</label>
      <div className="flex items-center gap-1.5 flex-wrap">
        <button
          type="button"
          title="Default"
          onClick={() => onChange(undefined)}
          className={`${swatchBase} ${value == null ? ringClass : idleClass}`}
          style={{ background: 'var(--chart-line)' }}
        />
        {COLORMAP_NAMES.map((name) => (
          <button
            key={name}
            type="button"
            title={name}
            onClick={() => onChange(name)}
            className={`${swatchBase} ${value === name ? ringClass : idleClass}`}
            style={{ background: buildColormapPreviewGradient(name) }}
          />
        ))}
        <label
          title="Custom color"
          className={`relative ${swatchBase} cursor-pointer overflow-hidden ${isCustom ? ringClass : idleClass}`}
          style={{ background: customSwatchColor }}
        >
          <input
            type="color"
            value={customSwatchColor}
            onChange={(e) => onChange(e.target.value)}
            className="absolute inset-0 w-full h-full opacity-0 cursor-pointer"
          />
        </label>
      </div>
      <HistogramColorPreview color={value} />
    </div>
  )
}

// =============================================================================
// Override Editor
// =============================================================================

export function OverrideEditor({
  overrides,
  availableColumns,
  availableColumnTypes,
  availableVariables = [],
  cellSelectionNames = [],
  onChange,
}: OverrideEditorProps) {
  const [isExpanded, setIsExpanded] = useState(overrides.length > 0)

  // Skip validation until we have query results (availableColumns is empty while query runs)
  const hasResults = availableColumns.length > 0

  const isHistogramTypedColumn = useCallback(
    (column: string) => {
      const type = availableColumnTypes?.[column]
      return !!type && isHistogramStructType(type)
    },
    [availableColumnTypes]
  )

  // Whether a card shows the "Render as" toggle at all: the selected column
  // is histogram-typed, or the card is already saved as kind: 'histogram'
  // (falls back to the stored kind whenever availableColumnTypes is empty or
  // missing the column, so a saved histogram card never silently reverts to
  // a plain Format textarea).
  const showsHistogramToggle = useCallback(
    (override: ColumnOverride) => isHistogramTypedColumn(override.column) || override.kind === 'histogram',
    [isHistogramTypedColumn]
  )

  // Validate all overrides for missing column references and unknown macros.
  // Gated on the card's effective kind being 'markdown' — a kind: 'histogram'
  // card may still carry a stale `format` from an earlier Markdown stint
  // (Design §2/§6 both preserve rather than clear it in different cases), and
  // that field isn't shown or editable while the card is in Histogram mode.
  const validationWarnings = useMemo(() => {
    return overrides.map((override) => {
      if (effectiveKind(override) !== 'markdown' || !hasResults) {
        return { missingColumns: [], unknownMacros: [] }
      }
      return validateFormatMacros(override.format ?? '', availableColumns, availableVariables, cellSelectionNames)
    })
  }, [overrides, availableColumns, availableVariables, cellSelectionNames, hasResults])

  // Check which overrides target columns not in the query results
  const availableColumnsSet = useMemo(() => new Set(availableColumns), [availableColumns])
  const isOrphanedColumn = useCallback(
    (column: string) => hasResults && !availableColumnsSet.has(column),
    [availableColumnsSet, hasResults]
  )

  const handleAddOverride = useCallback(() => {
    // Find first column not already overridden
    const usedColumns = new Set(overrides.map((o) => o.column))
    const firstAvailable = availableColumns.find((c) => !usedColumns.has(c)) || availableColumns[0]

    if (firstAvailable) {
      const newEntry: ColumnOverride = isHistogramTypedColumn(firstAvailable)
        ? { column: firstAvailable, kind: 'histogram' }
        : { column: firstAvailable, format: DEFAULT_FORMAT_TEMPLATE(firstAvailable) }
      onChange([...overrides, newEntry])
      setIsExpanded(true)
    }
  }, [overrides, availableColumns, isHistogramTypedColumn, onChange])

  const handleRemoveOverride = useCallback(
    (index: number) => {
      onChange(overrides.filter((_, i) => i !== index))
    },
    [overrides, onChange]
  )

  // Re-derives `kind` (and resets `format`/`histogramColor` as applicable)
  // from the newly selected column's type, per Design §2. Only acts when
  // that type is known (present in availableColumnTypes) — otherwise a
  // column change leaves kind/format/histogramColor untouched, same as
  // today's behavior, rather than guessing and risking an incorrect flip.
  const handleColumnChange = useCallback(
    (index: number, column: string) => {
      const newOverrides = [...overrides]
      const current = newOverrides[index]
      const type = availableColumnTypes?.[column]

      if (!type) {
        newOverrides[index] = { ...current, column }
        onChange(newOverrides)
        return
      }

      const newIsHistogram = isHistogramStructType(type)
      const currentKind = effectiveKind(current)

      if (newIsHistogram && currentKind !== 'histogram') {
        newOverrides[index] = { ...current, column, kind: 'histogram', format: undefined }
      } else if (!newIsHistogram && currentKind === 'histogram') {
        newOverrides[index] = {
          ...current,
          column,
          kind: 'markdown',
          histogramColor: undefined,
          format: DEFAULT_FORMAT_TEMPLATE(column),
        }
      } else {
        newOverrides[index] = { ...current, column }
      }
      onChange(newOverrides)
    },
    [overrides, availableColumnTypes, onChange]
  )

  const handleFormatChange = useCallback(
    (index: number, format: string) => {
      const newOverrides = [...overrides]
      newOverrides[index] = { ...newOverrides[index], format }
      onChange(newOverrides)
    },
    [overrides, onChange]
  )

  // Toggle's format/histogramColor side effects differ deliberately from
  // handleColumnChange's: the column hasn't changed, so any existing
  // template still refers to the right column and is preserved rather than
  // overwritten (Design §6's Editor UI).
  const handleKindChange = useCallback(
    (index: number, kind: 'markdown' | 'histogram') => {
      const newOverrides = [...overrides]
      const current = newOverrides[index]
      if (kind === 'markdown') {
        newOverrides[index] = {
          ...current,
          kind: 'markdown',
          histogramColor: undefined,
          format: current.format ?? DEFAULT_FORMAT_TEMPLATE(current.column),
        }
      } else {
        newOverrides[index] = { ...current, kind: 'histogram' }
      }
      onChange(newOverrides)
    },
    [overrides, onChange]
  )

  const handleHistogramColorChange = useCallback(
    (index: number, histogramColor: string | undefined) => {
      const newOverrides = [...overrides]
      newOverrides[index] = { ...newOverrides[index], histogramColor }
      onChange(newOverrides)
    },
    [overrides, onChange]
  )

  return (
    <div className="border-t border-theme-border">
      {/* Header */}
      <button
        onClick={() => setIsExpanded(!isExpanded)}
        className="w-full flex items-center justify-between px-4 py-2 bg-app-card hover:bg-app-card/80 transition-colors"
      >
        <div className="flex items-center gap-2">
          {isExpanded ? (
            <ChevronDown className="w-4 h-4 text-theme-text-muted" />
          ) : (
            <ChevronRight className="w-4 h-4 text-theme-text-muted" />
          )}
          <span className="text-sm font-semibold text-theme-text-primary">Overrides</span>
          {!isExpanded && overrides.length > 0 && (
            <span className="px-1.5 py-0.5 text-xs bg-accent-link/20 text-accent-link rounded-sm">
              {overrides.length}
            </span>
          )}
        </div>
      </button>

      {/* Content */}
      {isExpanded && (
        <div className="px-4 py-3 space-y-3">
          {overrides.map((override, index) => {
            const isOrphaned = isOrphanedColumn(override.column)
            const kind = effectiveKind(override)
            const showToggle = showsHistogramToggle(override)
            return (
              <div
                key={index}
                className={`p-3 bg-app-card rounded-md border ${isOrphaned ? 'border-amber-500/50' : 'border-theme-border'}`}
              >
                <div className="flex items-center justify-between mb-2">
                  <label className="text-xs font-medium text-theme-text-secondary">Column</label>
                  <button
                    onClick={() => handleRemoveOverride(index)}
                    className="p-1 text-theme-text-muted hover:text-accent-error transition-colors"
                    title="Remove override"
                  >
                    <X className="w-3.5 h-3.5" />
                  </button>
                </div>
                <select
                  value={override.column}
                  onChange={(e) => handleColumnChange(index, e.target.value)}
                  className={`w-full px-2 py-1.5 text-sm bg-app-bg border rounded-sm text-theme-text-primary mb-2 ${isOrphaned ? 'border-amber-500/50' : 'border-theme-border'}`}
                >
                  {/* Include orphaned column so it's visible and selectable */}
                  {isOrphaned && (
                    <option key={override.column} value={override.column}>
                      {override.column} (not in results)
                    </option>
                  )}
                  {availableColumns.map((col) => (
                    <option key={col} value={col}>
                      {col}
                    </option>
                  ))}
                </select>
                {isOrphaned && (
                  <div className="mb-2 flex items-start gap-1.5 text-xs text-amber-500">
                    <AlertTriangle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
                    <span>Column not in query results. Add it to the SELECT or choose a different column.</span>
                  </div>
                )}

                {showToggle && (
                  <div className="flex items-center gap-1.5 mb-2">
                    <span className="text-xs font-medium text-theme-text-secondary mr-1">Render as</span>
                    <button
                      type="button"
                      onClick={() => handleKindChange(index, 'markdown')}
                      className={`px-2 py-1 text-xs rounded-sm border transition-colors ${
                        kind === 'markdown'
                          ? 'bg-accent-link/20 border-accent-link text-accent-link'
                          : 'border-theme-border text-theme-text-secondary hover:text-theme-text-primary'
                      }`}
                    >
                      Markdown
                    </button>
                    <button
                      type="button"
                      onClick={() => handleKindChange(index, 'histogram')}
                      className={`px-2 py-1 text-xs rounded-sm border transition-colors ${
                        kind === 'histogram'
                          ? 'bg-accent-link/20 border-accent-link text-accent-link'
                          : 'border-theme-border text-theme-text-secondary hover:text-theme-text-primary'
                      }`}
                    >
                      Histogram
                    </button>
                  </div>
                )}

                {kind === 'histogram' ? (
                  <HistogramColorPicker
                    value={override.histogramColor}
                    onChange={(color) => handleHistogramColorChange(index, color)}
                  />
                ) : (
                  <>
                    <label className="block text-xs font-medium text-theme-text-secondary mb-1">Format</label>
                    <textarea
                      value={override.format ?? ''}
                      onChange={(e) => handleFormatChange(index, e.target.value)}
                      className="w-full px-2 py-1.5 text-sm bg-app-bg border border-theme-border rounded-sm text-theme-text-primary font-mono resize-y min-h-14"
                      rows={2}
                      placeholder="[View](/path?id=$row.column_name)"
                    />
                    {validationWarnings[index]?.unknownMacros.length > 0 && (
                      <div className="mt-1.5 flex items-start gap-1.5 text-xs text-amber-500">
                        <AlertTriangle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
                        <span>
                          Unknown macro{validationWarnings[index].unknownMacros.length > 1 ? 's' : ''}:{' '}
                          {validationWarnings[index].unknownMacros.map((macro, i) => (
                            <span key={macro}>
                              {i > 0 && ', '}
                              <code className="px-1 py-0.5 bg-amber-500/10 rounded-sm">{macro}</code>
                            </span>
                          ))}
                        </span>
                      </div>
                    )}
                    {validationWarnings[index]?.missingColumns.length > 0 && (
                      <div className="mt-1.5 flex items-start gap-1.5 text-xs text-amber-500">
                        <AlertTriangle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
                        <span>
                          Unknown column{validationWarnings[index].missingColumns.length > 1 ? 's' : ''}:{' '}
                          {validationWarnings[index].missingColumns.map((col, i) => (
                            <span key={col}>
                              {i > 0 && ', '}
                              <code className="px-1 py-0.5 bg-amber-500/10 rounded-sm">{col}</code>
                            </span>
                          ))}
                        </span>
                      </div>
                    )}
                  </>
                )}
              </div>
            )
          })}

          {/* Add button */}
          {availableColumns.length > 0 && (
            <button
              onClick={handleAddOverride}
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs text-accent-link hover:text-accent-link/80 transition-colors"
            >
              <Plus className="w-3.5 h-3.5" />
              Add Override
            </button>
          )}

          {/* Help text */}
          <div className="text-xs text-theme-text-muted space-y-1 pt-2 border-t border-theme-border">
            <div>
              Format: <code className="px-1 py-0.5 bg-theme-border rounded-sm">[label](url)</code>
            </div>
            <div>
              Row data: <code className="px-1 py-0.5 bg-theme-border rounded-sm">$row.name</code> or{' '}
              <code className="px-1 py-0.5 bg-theme-border rounded-sm">$row["column-name"]</code>
            </div>
            {cellSelectionNames.length > 0 && (
              <div>
                Selection:{' '}
                {cellSelectionNames.map((name, i) => (
                  <span key={name}>
                    {i > 0 && ', '}
                    <code className="px-1 py-0.5 bg-theme-border rounded-sm">${name}.selected.column</code>
                  </span>
                ))}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
