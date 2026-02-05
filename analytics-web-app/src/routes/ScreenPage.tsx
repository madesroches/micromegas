import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useParams, useNavigate, useSearchParams } from 'react-router-dom'
import { usePageTitle } from '@/hooks/usePageTitle'
import { AlertCircle, Save, GitCompareArrows } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { SaveScreenDialog } from '@/components/SaveScreenDialog'
import { ConfigDiffModal } from '@/components/ConfigDiffModal'
import { Button } from '@/components/ui/button'
import { parseTimeRange, getTimeRangeForApi } from '@/lib/time-range'
import { renderIcon } from '@/lib/screen-type-utils'
import { getRenderer } from '@/lib/screen-renderers/init'
import { DEFAULT_TIME_RANGE } from '@/lib/screen-defaults'
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

function ScreenPageContent() {
  const { name } = useParams<{ name: string }>()
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const isNew = !name

  // Screen state
  const [screen, setScreen] = useState<Screen | null>(null)

  // Read type directly from URL (only used for new screens)
  const typeParam = (searchParams.get('type') ?? null) as ScreenTypeName | null

  // Time range change handler - works directly with URL params to preserve variables
  // This avoids going through updateConfig which has stale variable state
  const handleTimeRangeChange = useCallback(
    (from: string, to: string) => {
      const params = new URLSearchParams(searchParams.toString())
      params.set('from', from)
      params.set('to', to)
      navigate(`?${params.toString()}`)
    },
    [searchParams, navigate]
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

  // Warn when URL query string exceeds safe length (older browsers/proxies may truncate)
  useEffect(() => {
    const qs = searchParams.toString()
    if (qs.length > 2000) {
      console.warn(
        `URL query string length (${qs.length}) exceeds safe threshold (2000). ` +
          `Some parameter values may be lost when sharing or bookmarking.`
      )
    }
  }, [searchParams])

  // Dialog state
  const [showSaveDialog, setShowSaveDialog] = useState(false)
  const [showDiffModal, setShowDiffModal] = useState(false)

  // Ref for the renderer's wrapped save handler (includes URL cleanup)
  const saveRef = useRef<(() => Promise<void>) | null>(null)

  // Refresh trigger - increment to tell renderer to re-execute query
  const [refreshTrigger, setRefreshTrigger] = useState(0)

  // Compute raw time range values (for renderer)
  // Priority: URL (if present) → saved config → current config
  const savedTimeFrom = screen?.config?.timeRangeFrom
  const savedTimeTo = screen?.config?.timeRangeTo
  const currentTimeFrom = screenConfig?.timeRangeFrom
  const currentTimeTo = screenConfig?.timeRangeTo
  // Compute raw time range - source of truth for displayed time
  // Priority: URL params → saved config → current config (from API)
  // Check each param individually since URL might only have one of from/to
  const rawTimeRange = useMemo(
    () => ({
      from: searchParams.get('from') ?? savedTimeFrom ?? currentTimeFrom!,
      to: searchParams.get('to') ?? savedTimeTo ?? currentTimeTo!,
    }),
    [searchParams, savedTimeFrom, savedTimeTo, currentTimeFrom, currentTimeTo]
  )

  // Compute parsed time range (for label)
  const parsedTimeRange = useMemo(() => {
    try {
      return parseTimeRange(rawTimeRange.from, rawTimeRange.to)
    } catch {
      // Fallback for invalid time range - use standard defaults
      return parseTimeRange(DEFAULT_TIME_RANGE.from, DEFAULT_TIME_RANGE.to)
    }
  }, [rawTimeRange])

  // Compute API time range
  const apiTimeRange = useMemo(() => {
    try {
      return getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to)
    } catch {
      // Fallback for invalid time range - use standard defaults
      return getTimeRangeForApi(DEFAULT_TIME_RANGE.from, DEFAULT_TIME_RANGE.to)
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

  // Save existing screen — API call + state update only; URL cleanup is the renderer's job
  const handleSave = useCallback(async (): Promise<ScreenConfig> => {
    if (!screen || !screenConfig) throw new Error('No screen to save')

    setIsSaving(true)
    setSaveError(null)

    // Save the displayed time range (rawTimeRange), not urlConfig which has defaults merged in
    // rawTimeRange correctly falls back: URL → saved → default
    const configToSave: ScreenConfig = {
      ...screenConfig,
      timeRangeFrom: rawTimeRange.from,
      timeRangeTo: rawTimeRange.to,
    }

    try {
      const updated = await updateScreen(screen.name, { config: configToSave })
      setScreen(updated)
      setScreenConfig(configToSave) // Keep local state in sync
      setHasUnsavedChanges(false)
      return configToSave
    } catch (err) {
      if (err instanceof ScreenApiError) {
        setSaveError(err.message)
      } else {
        setSaveError('Failed to save screen')
      }
      throw err
    } finally {
      setIsSaving(false)
    }
  }, [screen, screenConfig, rawTimeRange])

  // Title bar save handler - calls renderer's wrapped handler if available, else raw handleSave
  const handleTitleBarSave = useCallback(async () => {
    if (saveRef.current) {
      await saveRef.current()
    } else {
      await handleSave()
    }
  }, [handleSave])

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
              <div className="flex items-center gap-3 flex-wrap">
                <div className="p-2 rounded-md bg-app-card text-accent-link">
                  {renderIcon(screenTypeInfo?.icon ?? 'file-text')}
                </div>
                <div className="flex items-center gap-3">
                  <h1 className="text-2xl font-semibold text-theme-text-primary">
                    {isNew ? `New ${screenTypeInfo?.display_name ?? screenType} Screen` : screen?.name}
                  </h1>
                  {hasUnsavedChanges && (
                    <span className="text-sm" style={{ color: 'var(--accent-warning)' }}>
                      (unsaved changes)
                    </span>
                  )}
                </div>
                {/* Save controls */}
                <div className="flex items-center gap-2">
                  {hasUnsavedChanges && !isNew && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setShowDiffModal(true)}
                      className="gap-1"
                    >
                      <GitCompareArrows className="w-4 h-4" />
                      Diff
                    </Button>
                  )}
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setShowSaveDialog(true)}
                    className="gap-1"
                  >
                    <Save className="w-4 h-4" />
                    Save As
                  </Button>
                  {screen && (
                    <Button
                      size="sm"
                      onClick={handleTitleBarSave}
                      disabled={isSaving || !hasUnsavedChanges}
                      className="gap-1"
                    >
                      <Save className="w-4 h-4" />
                      {isSaving ? 'Saving...' : 'Save'}
                    </Button>
                  )}
                </div>
              </div>
              {isNew && (
                <p className="text-sm text-theme-text-secondary mt-1 ml-11">
                  {screenTypeInfo?.display_name ?? screenType}
                </p>
              )}
              {saveError && (
                <p className="text-xs text-accent-error mt-2 ml-11">{saveError}</p>
              )}
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
              onSaveRef={saveRef}
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

      {/* Config Diff Modal */}
      {screenConfig && (
        <ConfigDiffModal
          isOpen={showDiffModal}
          onClose={() => setShowDiffModal(false)}
          savedConfig={screen?.config ?? null}
          currentConfig={screenConfig}
          currentTimeRange={rawTimeRange}
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
