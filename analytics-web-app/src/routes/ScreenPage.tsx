import { Suspense, useState, useCallback, useMemo, useEffect } from 'react'
import { useParams, useNavigate, useSearchParams } from 'react-router-dom'
import { usePageTitle } from '@/hooks/usePageTitle'
import { AlertCircle } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { SaveScreenDialog } from '@/components/SaveScreenDialog'
import { useScreenConfig } from '@/hooks/useScreenConfig'
import { parseTimeRange, getTimeRangeForApi } from '@/lib/time-range'
import { isReservedParam } from '@/lib/url-params'
import { renderIcon } from '@/lib/screen-type-utils'
import { getRenderer } from '@/lib/screen-renderers/init'
import { DEFAULT_SCREEN_TIME_RANGE } from '@/lib/screen-defaults'
import type { ScreenPageConfig } from '@/lib/screen-config'
import {
  getScreen,
  getScreenTypes,
  getDefaultConfig,
  updateScreen,
  Screen,
  ScreenTypeName,
  ScreenTypeInfo,
  ScreenConfig,
  ScreenApiError,
} from '@/lib/screens-api'

// Default config for ScreenPage (used when no saved config exists)
const DEFAULT_CONFIG: ScreenPageConfig = {
  timeRangeFrom: DEFAULT_SCREEN_TIME_RANGE.from,
  timeRangeTo: DEFAULT_SCREEN_TIME_RANGE.to,
  type: undefined,
  variables: {},
}

// Safe URL length threshold (conservative for older browsers/proxies)
const MAX_SAFE_URL_LENGTH = 2000

// URL builder factory - creates buildUrl with saved config as baseline for delta calculations
// URL should only contain values that differ from the saved config (not hardcoded defaults)
const createBuildUrl = (savedConfig: ScreenConfig | null) => {
  // Extract saved time range, falling back to defaults for new screens
  const savedTimeFrom = (savedConfig as { timeRangeFrom?: string } | null)?.timeRangeFrom ?? DEFAULT_CONFIG.timeRangeFrom
  const savedTimeTo = (savedConfig as { timeRangeTo?: string } | null)?.timeRangeTo ?? DEFAULT_CONFIG.timeRangeTo

  return (cfg: ScreenPageConfig): string => {
    const params = new URLSearchParams()
    if (cfg.type) params.set('type', cfg.type)

    // Only serialize time range if it differs from saved config baseline
    if (cfg.timeRangeFrom && cfg.timeRangeFrom !== savedTimeFrom) {
      params.set('from', cfg.timeRangeFrom)
    }
    if (cfg.timeRangeTo && cfg.timeRangeTo !== savedTimeTo) {
      params.set('to', cfg.timeRangeTo)
    }

    // Add variable params (skip reserved names as safety check)
    // Note: empty strings ARE serialized (as ?name=) to preserve explicit "cleared" state
    if (cfg.variables) {
      for (const [name, value] of Object.entries(cfg.variables)) {
        if (value !== undefined && !isReservedParam(name)) {
          params.set(name, value)
        }
      }
    }

    const qs = params.toString()
    const url = qs ? `?${qs}` : ''

    // Warn if URL exceeds safe length (variables may be lost on some browsers/proxies)
    if (url.length > MAX_SAFE_URL_LENGTH) {
      console.warn(
        `URL length (${url.length}) exceeds safe threshold (${MAX_SAFE_URL_LENGTH}). ` +
          `Some variable values may be lost when sharing or bookmarking.`
      )
    }

    return url
  }
}

function ScreenPageContent() {
  const { name } = useParams<{ name: string }>()
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const isNew = !name

  // Screen state (declared early so buildUrl can use saved config)
  const [screen, setScreen] = useState<Screen | null>(null)

  // Create buildUrl with saved config as baseline for delta calculations
  // URL only contains values that differ from saved config
  const buildUrl = useMemo(() => createBuildUrl(screen?.config ?? null), [screen?.config])

  // Use the config-driven pattern for URL state (time range, type for new screens)
  const { config: urlConfig, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl)
  const typeParam = (urlConfig.type ?? null) as ScreenTypeName | null

  // Time range change handler (navigational)
  const handleTimeRangeChange = useCallback(
    (from: string, to: string) => {
      updateConfig({ timeRangeFrom: from, timeRangeTo: to })
    },
    [updateConfig]
  )

  // Variable change handler (replace to avoid cluttering history)
  const handleUrlVariableChange = useCallback(
    (name: string, value: string) => {
      updateConfig(
        {
          variables: { ...(urlConfig.variables || {}), [name]: value },
        },
        { replace: true }
      )
    },
    [updateConfig, urlConfig.variables]
  )

  // Variable remove handler (cleans up orphaned URL params when variable cell is deleted)
  const handleUrlVariableRemove = useCallback(
    (name: string) => {
      const newVariables = { ...(urlConfig.variables || {}) }
      delete newVariables[name]
      updateConfig({ variables: newVariables }, { replace: true })
    },
    [updateConfig, urlConfig.variables]
  )

  // Screen type info (fetched from API)
  const [screenTypeInfo, setScreenTypeInfo] = useState<ScreenTypeInfo | null>(null)

  // Page title - show screen name or "New [type] Screen"
  const pageTitle = isNew
    ? (screenTypeInfo ? `New ${screenTypeInfo.display_name} Screen` : null)
    : screen?.name ?? null
  usePageTitle(pageTitle)
  const [screenConfig, setScreenConfig] = useState<ScreenConfig | null>(null)
  const [screenType, setScreenType] = useState<ScreenTypeName | null>(null)
  const [isLoading, setIsLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [hasUnsavedChanges, setHasUnsavedChanges] = useState(false)
  const [isSaving, setIsSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)

  // Dialog state
  const [showSaveDialog, setShowSaveDialog] = useState(false)

  // Refresh trigger - increment to tell renderer to re-execute query
  const [refreshTrigger, setRefreshTrigger] = useState(0)

  // Compute raw time range values (for renderer)
  // Priority: URL (if present) → saved config → current config
  // Check actual URL params to detect explicit overrides (not merged urlConfig which always has defaults)
  const urlHasTimeRange = searchParams.has('from') || searchParams.has('to')
  const savedTimeFrom = (screen?.config as { timeRangeFrom?: string } | undefined)?.timeRangeFrom
  const savedTimeTo = (screen?.config as { timeRangeTo?: string } | undefined)?.timeRangeTo
  const currentTimeFrom = (screenConfig as { timeRangeFrom?: string } | null)?.timeRangeFrom
  const currentTimeTo = (screenConfig as { timeRangeTo?: string } | null)?.timeRangeTo
  const rawTimeRange = useMemo(
    () => ({
      from: urlHasTimeRange
        ? urlConfig.timeRangeFrom!
        : (savedTimeFrom ?? currentTimeFrom ?? DEFAULT_CONFIG.timeRangeFrom!),
      to: urlHasTimeRange
        ? urlConfig.timeRangeTo!
        : (savedTimeTo ?? currentTimeTo ?? DEFAULT_CONFIG.timeRangeTo!),
    }),
    [urlHasTimeRange, urlConfig.timeRangeFrom, urlConfig.timeRangeTo, savedTimeFrom, savedTimeTo, currentTimeFrom, currentTimeTo]
  )

  // Compute parsed time range (for label)
  const parsedTimeRange = useMemo(() => {
    try {
      return parseTimeRange(rawTimeRange.from, rawTimeRange.to)
    } catch {
      return parseTimeRange(DEFAULT_CONFIG.timeRangeFrom!, DEFAULT_CONFIG.timeRangeTo!)
    }
  }, [rawTimeRange])

  // Compute API time range
  const apiTimeRange = useMemo(() => {
    try {
      return getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to)
    } catch {
      return getTimeRangeForApi(DEFAULT_CONFIG.timeRangeFrom!, DEFAULT_CONFIG.timeRangeTo!)
    }
  }, [rawTimeRange])

  // Load screen or default config
  useEffect(() => {
    async function load() {
      setIsLoading(true)
      setLoadError(null)
      setHasUnsavedChanges(false)
      setScreen(null)

      try {
        // Fetch screen types for display info
        const types = await getScreenTypes()
        const typeMap = new Map(types.map((t) => [t.name, t]))

        if (isNew) {
          // New screen - load default config for the type
          if (!typeParam) {
            setLoadError('No screen type specified')
            return
          }
          const defaultConfig = await getDefaultConfig(typeParam)
          setScreenConfig(defaultConfig)
          setScreenType(typeParam)
          setScreenTypeInfo(typeMap.get(typeParam) ?? null)
        } else {
          // Existing screen - load from API
          const loadedScreen = await getScreen(name)
          setScreen(loadedScreen)
          setScreenConfig(loadedScreen.config)
          setScreenType(loadedScreen.screen_type as ScreenTypeName)
          setScreenTypeInfo(typeMap.get(loadedScreen.screen_type as ScreenTypeName) ?? null)
          // Note: We don't push saved time range to URL here.
          // rawTimeRange falls back to saved config, and buildUrl compares against saved config.
          // URL only contains time params that differ from saved config (delta-based).
        }
      } catch (err) {
        if (err instanceof ScreenApiError) {
          if (err.code === 'NOT_FOUND') {
            setLoadError(`Screen "${name}" not found`)
          } else {
            setLoadError(err.message)
          }
        } else {
          setLoadError('Failed to load screen')
        }
      } finally {
        setIsLoading(false)
      }
    }

    load()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isNew, name, typeParam])

  // Handle config changes from renderer - supports both direct config and updater function
  const handleScreenConfigChange = useCallback(
    (configOrUpdater: ScreenConfig | ((prev: ScreenConfig) => ScreenConfig)) => {
      setScreenConfig((prev) => {
        if (typeof configOrUpdater === 'function') {
          // Functional update - updater receives current state, returns new state
          return configOrUpdater(prev ?? ({} as ScreenConfig))
        }
        // Direct update - merge with previous (backwards compatible)
        return prev ? { ...prev, ...configOrUpdater } : configOrUpdater
      })
    },
    []
  )

  // Handle refresh button click
  const handleRefresh = useCallback(() => {
    setRefreshTrigger((n) => n + 1)
  }, [])

  // Save existing screen
  const handleSave = useCallback(async () => {
    if (!screen || !screenConfig) return

    setIsSaving(true)
    setSaveError(null)

    // Merge URL time range into config for save
    // This ensures we save what the user sees, even if the async effect hasn't synced yet
    const configToSave = {
      ...screenConfig,
      timeRangeFrom: urlConfig.timeRangeFrom ?? screenConfig.timeRangeFrom,
      timeRangeTo: urlConfig.timeRangeTo ?? screenConfig.timeRangeTo,
    }

    try {
      const updated = await updateScreen(screen.name, { config: configToSave })
      setScreen(updated)
      setScreenConfig(configToSave) // Keep local state in sync
      setHasUnsavedChanges(false)

      // Clean up URL params that now match saved values (delta-based URL)
      // After save, the saved config becomes the new baseline for delta calculations
      // We use navigate directly because buildUrl hasn't updated yet (state change pending)
      const currentUrlVars = urlConfig.variables || {}
      const variablesToRemove: string[] = []

      // Check each URL variable against the newly saved config
      const savedCells = (configToSave as { cells?: Array<{ type: string; name: string; defaultValue?: string }> })
        .cells
      if (savedCells) {
        for (const [name, value] of Object.entries(currentUrlVars)) {
          const savedCell = savedCells.find((c) => c.type === 'variable' && c.name === name)
          if (savedCell && savedCell.defaultValue === value) {
            variablesToRemove.push(name)
          }
        }
      }

      // Build clean URL with only params that differ from newly saved config
      const newSavedTimeFrom = (configToSave as { timeRangeFrom?: string }).timeRangeFrom
      const newSavedTimeTo = (configToSave as { timeRangeTo?: string }).timeRangeTo
      const params = new URLSearchParams()

      // Time range: only include if differs from saved (which it shouldn't after save)
      if (urlConfig.timeRangeFrom && urlConfig.timeRangeFrom !== newSavedTimeFrom) {
        params.set('from', urlConfig.timeRangeFrom)
      }
      if (urlConfig.timeRangeTo && urlConfig.timeRangeTo !== newSavedTimeTo) {
        params.set('to', urlConfig.timeRangeTo)
      }

      // Variables: only include if differs from saved
      const newVariables = { ...currentUrlVars }
      for (const name of variablesToRemove) {
        delete newVariables[name]
      }
      for (const [name, value] of Object.entries(newVariables)) {
        if (value !== undefined && !isReservedParam(name)) {
          params.set(name, value)
        }
      }

      const qs = params.toString()
      // Use '.' to navigate to current location without query params
      // (window.location.pathname includes base path, which navigate() would duplicate)
      navigate(qs ? `?${qs}` : '.', { replace: true })
    } catch (err) {
      if (err instanceof ScreenApiError) {
        setSaveError(err.message)
      } else {
        setSaveError('Failed to save screen')
      }
    } finally {
      setIsSaving(false)
    }
  }, [screen, screenConfig, urlConfig.variables, urlConfig.timeRangeFrom, urlConfig.timeRangeTo, navigate])

  // Handle "Save As" completion
  const handleSaveAsComplete = useCallback(
    (newName: string) => {
      setShowSaveDialog(false)
      navigate(`/screen/${newName}`)
    },
    [navigate]
  )

  const currentValues = useMemo(
    () => ({
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    }),
    [apiTimeRange]
  )

  // Loading state - also check if loaded screen matches URL
  const isLoadingScreen = isLoading || (!isNew && screen?.name !== name)
  if (isLoadingScreen) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex items-center justify-center h-64">
            <div className="flex items-center gap-3">
              <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
              <span className="text-theme-text-secondary">Loading screen...</span>
            </div>
          </div>
        </div>
      </PageLayout>
    )
  }

  // Error state
  if (loadError) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-app-panel border border-theme-border rounded-lg">
            <AlertCircle className="w-10 h-10 text-accent-error mb-3" />
            <p className="text-theme-text-secondary">{loadError}</p>
            <AppLink href="/screens" className="text-accent-link hover:underline mt-2">
              Back to Screens
            </AppLink>
          </div>
        </div>
      </PageLayout>
    )
  }

  // Missing config
  if (!screenConfig || !screenType) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-app-panel border border-theme-border rounded-lg">
            <AlertCircle className="w-10 h-10 text-accent-error mb-3" />
            <p className="text-theme-text-secondary">Failed to load screen configuration</p>
            <AppLink href="/screens" className="text-accent-link hover:underline mt-2">
              Back to Screens
            </AppLink>
          </div>
        </div>
      </PageLayout>
    )
  }

  // Get renderer for this screen type
  const Renderer = getRenderer(screenType)

  if (!Renderer) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-app-panel border border-theme-border rounded-lg">
            <AlertCircle className="w-10 h-10 text-accent-error mb-3" />
            <p className="text-theme-text-secondary">Unknown screen type: {screenType}</p>
            <AppLink href="/screens" className="text-accent-link hover:underline mt-2">
              Back to Screens
            </AppLink>
          </div>
        </div>
      </PageLayout>
    )
  }

  return (
    <>
      <PageLayout
        onRefresh={handleRefresh}
        timeRangeControl={{
          timeRangeFrom: rawTimeRange.from,
          timeRangeTo: rawTimeRange.to,
          onTimeRangeChange: handleTimeRangeChange,
        }}
      >
        <div className="flex flex-col h-full">
          {/* Header */}
          <div className="p-6 pb-0">
            <div className="mb-5">
              <div className="flex items-center gap-3">
                <div className="p-2 rounded-md bg-app-card text-accent-link">
                  {renderIcon(screenTypeInfo?.icon ?? 'file-text')}
                </div>
                <div>
                  <h1 className="text-2xl font-semibold text-theme-text-primary">
                    {isNew ? `New ${screenTypeInfo?.display_name ?? screenType} Screen` : screen?.name}
                  </h1>
                  {(isNew || hasUnsavedChanges) && (
                    <p className="text-sm text-theme-text-secondary">
                      {isNew && (screenTypeInfo?.display_name ?? screenType)}
                      {hasUnsavedChanges && (
                        <span className={isNew ? 'ml-2' : ''} style={{ color: 'var(--accent-warning)' }}>
                          (unsaved changes)
                        </span>
                      )}
                    </p>
                  )}
                </div>
              </div>
            </div>
          </div>

          {/* Renderer - key forces remount when screen changes */}
          <div className="flex-1 min-h-0">
            <Renderer
              key={screen?.name ?? 'new'}
              config={screenConfig}
              onConfigChange={handleScreenConfigChange}
              savedConfig={screen?.config ?? null}
              setHasUnsavedChanges={setHasUnsavedChanges}
              timeRange={apiTimeRange}
              rawTimeRange={rawTimeRange}
              onTimeRangeChange={handleTimeRangeChange}
              timeRangeLabel={parsedTimeRange.label}
              currentValues={currentValues}
              onSave={screen ? handleSave : null}
              isSaving={isSaving}
              hasUnsavedChanges={hasUnsavedChanges}
              onSaveAs={() => setShowSaveDialog(true)}
              saveError={saveError}
              refreshTrigger={refreshTrigger}
              urlVariables={urlConfig.variables || {}}
              onUrlVariableChange={handleUrlVariableChange}
              onUrlVariableRemove={handleUrlVariableRemove}
            />
          </div>
        </div>
      </PageLayout>

      {/* Save As Dialog */}
      {screenConfig && screenType && (
        <SaveScreenDialog
          isOpen={showSaveDialog}
          onClose={() => setShowSaveDialog(false)}
          onSaved={handleSaveAsComplete}
          screenType={screenType}
          config={screenConfig}
          suggestedName={screen?.name ? `${screen.name}-copy` : undefined}
        />
      )}
    </>
  )
}

export default function ScreenPage() {
  return (
    <AuthGuard>
      <Suspense
        fallback={
          <PageLayout>
            <div className="p-6">
              <div className="flex items-center justify-center h-64">
                <div className="animate-spin rounded-full h-8 w-8 border-2 border-accent-link border-t-transparent" />
              </div>
            </div>
          </PageLayout>
        }
      >
        <ScreenPageContent />
      </Suspense>
    </AuthGuard>
  )
}
