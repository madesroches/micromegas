import { Suspense, useState, useCallback, useMemo, useEffect } from 'react'
import { useParams, useSearchParams, useNavigate } from 'react-router-dom'
import { AlertCircle } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { SaveScreenDialog } from '@/components/SaveScreenDialog'
import { useTimeRange } from '@/hooks/useTimeRange'
import { renderIcon } from '@/lib/screen-type-utils'
import { getRenderer } from '@/lib/screen-renderers/init'
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
  const [searchParams] = useSearchParams()
  const navigate = useNavigate()
  const isNew = !name
  const typeParam = searchParams.get('type') as ScreenTypeName | null

  const { timeRange: rawTimeRange, parsed: timeRange, apiTimeRange, setTimeRange } = useTimeRange()

  // Track expected time range for sync detection (null = no sync needed)
  const [expectedTimeRange, setExpectedTimeRange] = useState<{ from: string; to: string } | null>(null)

  // Screen state
  const [screen, setScreen] = useState<Screen | null>(null)
  const [config, setConfigState] = useState<ScreenConfig | null>(null)
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

  // Screen type info (fetched from API)
  const [screenTypeInfo, setScreenTypeInfo] = useState<ScreenTypeInfo | null>(null)

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
          setConfigState(defaultConfig)
          setScreenType(typeParam)
          setScreenTypeInfo(typeMap.get(typeParam) ?? null)
        } else {
          // Existing screen - load from API
          const loadedScreen = await getScreen(name)
          setScreen(loadedScreen)
          setConfigState(loadedScreen.config)
          setScreenType(loadedScreen.screen_type as ScreenTypeName)
          setScreenTypeInfo(typeMap.get(loadedScreen.screen_type as ScreenTypeName) ?? null)

          // Initialize time range from saved config (if present and no URL params)
          const savedTimeRangeFrom = loadedScreen.config.timeRangeFrom as string | undefined
          const savedTimeRangeTo = loadedScreen.config.timeRangeTo as string | undefined
          if (savedTimeRangeFrom && savedTimeRangeTo) {
            // Only apply if URL doesn't already have time params
            const urlHasTimeParams = searchParams.has('from') || searchParams.has('to')
            if (!urlHasTimeParams) {
              // Track expected values so we can wait for sync
              setExpectedTimeRange({ from: savedTimeRangeFrom, to: savedTimeRangeTo })
              setTimeRange(savedTimeRangeFrom, savedTimeRangeTo)
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

  // Clear expectedTimeRange once rawTimeRange has synced
  useEffect(() => {
    if (
      expectedTimeRange &&
      rawTimeRange.from === expectedTimeRange.from &&
      rawTimeRange.to === expectedTimeRange.to
    ) {
      setExpectedTimeRange(null)
    }
  }, [rawTimeRange, expectedTimeRange])

  // Handle config changes from renderer - MERGE to avoid race conditions
  const handleConfigChange = useCallback((partialConfig: ScreenConfig) => {
    setConfigState(prev => (prev ? { ...prev, ...partialConfig } : partialConfig))
  }, [])

  // Handle unsaved changes notification from renderer
  const handleUnsavedChange = useCallback(() => {
    setHasUnsavedChanges(true)
  }, [])

  // Handle time range changes from renderer
  const handleTimeRangeChange = useCallback(
    (from: string, to: string) => {
      setTimeRange(from, to)
    },
    [setTimeRange]
  )

  // Handle refresh button click
  const handleRefresh = useCallback(() => {
    setRefreshTrigger((n) => n + 1)
  }, [])

  // Save existing screen
  const handleSave = useCallback(async () => {
    if (!screen || !config) return

    setIsSaving(true)
    setSaveError(null)

    try {
      const updated = await updateScreen(screen.name, { config })
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
  }, [screen, config])

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

  // Loading state - also check if loaded screen matches URL and time range is synced
  const isLoadingScreen = isLoading || (!isNew && screen?.name !== name) || expectedTimeRange !== null
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
  if (!config || !screenType) {
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
      <PageLayout onRefresh={handleRefresh}>
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
              config={config}
              onConfigChange={handleConfigChange}
              savedConfig={screen?.config ?? null}
              onUnsavedChange={handleUnsavedChange}
              timeRange={apiTimeRange}
              rawTimeRange={rawTimeRange}
              onTimeRangeChange={handleTimeRangeChange}
              timeRangeLabel={timeRange.label}
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
      {config && screenType && (
        <SaveScreenDialog
          isOpen={showSaveDialog}
          onClose={() => setShowSaveDialog(false)}
          onSaved={handleSaveAsComplete}
          screenType={screenType}
          config={config}
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
