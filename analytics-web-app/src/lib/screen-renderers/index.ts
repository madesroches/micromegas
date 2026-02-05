import { ComponentType, MutableRefObject } from 'react'
import { Table } from 'apache-arrow'
import { ScreenConfig, ScreenTypeName } from '@/lib/screens-api'

// Re-export for convenience
export type { ScreenConfig, ScreenTypeName }

/**
 * Props passed to all screen renderers.
 *
 * Renderers own their full UI including:
 * - Query execution (if needed)
 * - Layout (panels, editors, etc.)
 * - Type-specific state (sorting, scale mode, etc.)
 */
export interface ScreenRendererProps {
  /** Current config (opaque to parent, renderer interprets it) */
  config: ScreenConfig
  /** Update config - accepts new config or updater function for atomic updates */
  onConfigChange: (
    configOrUpdater: ScreenConfig | ((prev: ScreenConfig) => ScreenConfig)
  ) => void
  /** Saved config from database, null if new screen - for unsaved detection */
  savedConfig: ScreenConfig | null
  /** Set unsaved changes state (true when config differs from saved) */
  setHasUnsavedChanges: (value: boolean) => void
  /** Time range for API queries (ISO timestamps) */
  timeRange: { begin: string; end: string }
  /** Raw time range from URL (e.g., 'now-1h', 'now') */
  rawTimeRange: { from: string; to: string }
  /** Update URL time range (e.g., from chart drag-to-zoom) */
  onTimeRangeChange: (from: string, to: string) => void
  /** Time range display label */
  timeRangeLabel: string
  /** Current values for SQL variables */
  currentValues: Record<string, string>
  /** Parent's save handler (for existing screens). Returns saved config for post-save cleanup. */
  onSave: (() => Promise<ScreenConfig>) | null
  /** Whether save is in progress */
  isSaving: boolean
  /** Whether there are unsaved changes */
  hasUnsavedChanges: boolean
  /** Open save-as dialog */
  onSaveAs: () => void
  /** Save error message */
  saveError: string | null
  /** Increment to trigger a refresh (re-execute query) */
  refreshTrigger: number
  /** Ref for the renderer's wrapped save handler (includes URL cleanup). Title bar calls this. */
  onSaveRef?: MutableRefObject<(() => Promise<void>) | null>
}

/**
 * Common props for data display components within renderers.
 * Renderers can use this for their internal components.
 */
export interface DataDisplayProps {
  table: Table | null
  isLoading: boolean
  error: string | null
}

// Registry populated by renderer imports
export const SCREEN_RENDERERS: Record<string, ComponentType<ScreenRendererProps>> = {}

/**
 * Register a renderer for a screen type.
 * Called by each renderer module on import.
 */
export function registerRenderer(
  typeName: ScreenTypeName,
  component: ComponentType<ScreenRendererProps>
): void {
  SCREEN_RENDERERS[typeName] = component
}

/**
 * Get a renderer for a screen type.
 * Returns undefined if no renderer is registered.
 */
export function getRenderer(
  typeName: string
): ComponentType<ScreenRendererProps> | undefined {
  return SCREEN_RENDERERS[typeName]
}
