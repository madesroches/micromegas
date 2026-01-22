import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useParams, useNavigate } from 'react-router-dom'
import { usePageTitle } from '@/hooks/usePageTitle'
import { AlertCircle } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { SaveScreenDialog } from '@/components/SaveScreenDialog'
import { useScreenConfig } from '@/hooks/useScreenConfig'
import { parseTimeRange, getTimeRangeForApi } from '@/lib/time-range'
import { renderIcon } from '@/lib/screen-type-utils'
import { getRenderer } from '@/lib/screen-renderers/init'
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

// Default config for ScreenPage
const DEFAULT_CONFIG: ScreenPageConfig = {
  timeRangeFrom: 'now-1h',
  timeRangeTo: 'now',
  type: undefined,
}

// URL builder for ScreenPage
const buildUrl = (cfg: ScreenPageConfig): string => {
  const params = new URLSearchParams()
  if (cfg.type) params.set('type', cfg.type)
  if (cfg.timeRangeFrom && cfg.timeRangeFrom !== DEFAULT_CONFIG.timeRangeFrom) {
    params.set('from', cfg.timeRangeFrom)
  }
  if (cfg.timeRangeTo && cfg.timeRangeTo !== DEFAULT_CONFIG.timeRangeTo) {
    params.set('to', cfg.timeRangeTo)
  }
  const qs = params.toString()
  return qs ? `?${qs}` : ''
}

function ScreenPageContent() {
  const { name } = useParams<{ name: string }>()
  const navigate = useNavigate()
  const isNew = !name

  // Use the config-driven pattern for URL state (time range, type for new screens)
  const { config: urlConfig, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl)
  const typeParam = (urlConfig.type ?? null) as ScreenTypeName | null

  // Track if we've applied saved time range (to avoid re-applying on subsequent renders)
  const hasAppliedSavedTimeRangeRef = useRef(false)

  // Compute raw time range values (for renderer)
  const rawTimeRange = useMemo(
    () => ({
      from: urlConfig.timeRangeFrom ?? 'now-1h',
      to: urlConfig.timeRangeTo ?? 'now',
    }),
    [urlConfig.timeRangeFrom, urlConfig.timeRangeTo]
  )

  // Compute parsed time range (for label)
  const parsedTimeRange = useMemo(() => {
    try {
      return parseTimeRange(rawTimeRange.from, rawTimeRange.to)
    } catch {
      return parseTimeRange('now-1h', 'now')
    }
  }, [rawTimeRange])

  // Compute API time range
  const apiTimeRange = useMemo(() => {
    try {
      return getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to)
    } catch {
      return getTimeRangeForApi('now-1h', 'now')
    }
  }, [rawTimeRange])

  // Time range change handler (navigational)
  const handleTimeRangeChange = useCallback(
    (from: string, to: string) => {
      updateConfig({ timeRangeFrom: from, timeRangeTo: to })
    },
    [updateConfig]
  )

  // Screen state
  const [screen, setScreen] = useState<Screen | null>(null)

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

  // Load screen or default config
  useEffect(() => {
    async function load() {
      setIsLoading(true)
      setLoadError(null)
      setHasUnsavedChanges(false)
      setScreen(null)
      hasAppliedSavedTimeRangeRef.current = false

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

          // Initialize time range from saved config (if present and URL uses defaults)
          const savedTimeRangeFrom = loadedScreen.config.timeRangeFrom as string | undefined
          const savedTimeRangeTo = loadedScreen.config.timeRangeTo as string | undefined
          if (savedTimeRangeFrom && savedTimeRangeTo && !hasAppliedSavedTimeRangeRef.current) {
            // Only apply if URL has default time range (not explicitly set by user)
            const urlHasCustomTimeRange =
              (urlConfig.timeRangeFrom && urlConfig.timeRangeFrom !== DEFAULT_CONFIG.timeRangeFrom) ||
              (urlConfig.timeRangeTo && urlConfig.timeRangeTo !== DEFAULT_CONFIG.timeRangeTo)
            if (!urlHasCustomTimeRange) {
              hasAppliedSavedTimeRangeRef.current = true
              // Apply saved time range (replace to avoid creating history entry)
              updateConfig({ timeRangeFrom: savedTimeRangeFrom, timeRangeTo: savedTimeRangeTo }, { replace: true })
            }
          }
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

  // Handle unsaved changes notification from renderer
  const handleUnsavedChange = useCallback(() => {
    setHasUnsavedChanges(true)
  }, [])

  // Handle refresh button click
  const handleRefresh = useCallback(() => {
    setRefreshTrigger((n) => n + 1)
  }, [])

  // Save existing screen
  const handleSave = useCallback(async () => {
    if (!screen || !screenConfig) return

    setIsSaving(true)
    setSaveError(null)

    try {
      const updated = await updateScreen(screen.name, { config: screenConfig })
      setScreen(updated)
      setHasUnsavedChanges(false)
    } catch (err) {
      if (err instanceof ScreenApiError) {
        setSaveError(err.message)
      } else {
        setSaveError('Failed to save screen')
      }
    } finally {
      setIsSaving(false)
    }
  }, [screen, screenConfig])

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
              onUnsavedChange={handleUnsavedChange}
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
