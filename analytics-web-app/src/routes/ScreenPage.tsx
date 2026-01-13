import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useParams, useSearchParams, useNavigate } from 'react-router-dom'
import { AlertCircle, List, LineChart, FileText, Save } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { ErrorBanner } from '@/components/ErrorBanner'
import { QueryEditor } from '@/components/QueryEditor'
import { Button } from '@/components/ui/button'
import { AppLink } from '@/components/AppLink'
import { SaveScreenDialog } from '@/components/SaveScreenDialog'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useTimeRange } from '@/hooks/useTimeRange'
import {
  getScreen,
  getDefaultConfig,
  updateScreen,
  Screen,
  ScreenTypeName,
  ScreenConfig,
  ScreenApiError,
} from '@/lib/screens-api'

// Get display name for screen type
function getScreenTypeDisplayName(typeName: ScreenTypeName): string {
  switch (typeName) {
    case 'process_list':
      return 'Process List'
    case 'metrics':
      return 'Metrics'
    case 'log':
      return 'Log'
    default:
      return typeName
  }
}

// Get icon for screen type
function getScreenTypeIcon(typeName: ScreenTypeName) {
  switch (typeName) {
    case 'process_list':
      return <List className="w-5 h-5" />
    case 'metrics':
      return <LineChart className="w-5 h-5" />
    case 'log':
      return <FileText className="w-5 h-5" />
    default:
      return <FileText className="w-5 h-5" />
  }
}

// Variables available for all screen types
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
]

function ScreenPageContent() {
  const { name } = useParams<{ name: string }>()
  const [searchParams] = useSearchParams()
  const navigate = useNavigate()
  const isNew = !name
  const typeParam = searchParams.get('type') as ScreenTypeName | null

  const { parsed: timeRange, apiTimeRange } = useTimeRange()

  // Screen state
  const [screen, setScreen] = useState<Screen | null>(null)
  const [config, setConfig] = useState<ScreenConfig | null>(null)
  const [screenType, setScreenType] = useState<ScreenTypeName | null>(null)
  const [isLoading, setIsLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [hasUnsavedChanges, setHasUnsavedChanges] = useState(false)
  const [isSaving, setIsSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)

  // Dialog state
  const [showSaveDialog, setShowSaveDialog] = useState(false)

  // Query state
  const streamQuery = useStreamQuery()
  const queryError = streamQuery.error?.message ?? null
  const table = streamQuery.getTable()

  const currentSqlRef = useRef<string>('')
  const executeRef = useRef(streamQuery.execute)
  executeRef.current = streamQuery.execute

  // Load screen or default config
  useEffect(() => {
    async function load() {
      setIsLoading(true)
      setLoadError(null)

      try {
        if (isNew) {
          // New screen - load default config for the type
          if (!typeParam) {
            setLoadError('No screen type specified')
            return
          }
          const defaultConfig = await getDefaultConfig(typeParam)
          setConfig(defaultConfig)
          setScreenType(typeParam)
          currentSqlRef.current = defaultConfig.sql
        } else {
          // Existing screen - load from API
          const loadedScreen = await getScreen(name)
          setScreen(loadedScreen)
          setConfig(loadedScreen.config)
          setScreenType(loadedScreen.screen_type as ScreenTypeName)
          currentSqlRef.current = loadedScreen.config.sql
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
  }, [isNew, name, typeParam])

  // Execute query when config is loaded
  const loadData = useCallback(
    (sql: string) => {
      if (!screenType) return
      currentSqlRef.current = sql
      setConfig((prev) => (prev ? { ...prev, sql } : null))

      // Check if SQL changed from saved version
      if (screen && sql !== screen.config.sql) {
        setHasUnsavedChanges(true)
      }

      const params: Record<string, string> = {
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      }

      executeRef.current({
        sql,
        params,
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    },
    [screenType, screen, apiTimeRange]
  )

  // Initial query execution
  const hasExecutedRef = useRef(false)
  useEffect(() => {
    if (config && !hasExecutedRef.current && !isLoading) {
      hasExecutedRef.current = true
      loadData(config.sql)
    }
  }, [config, isLoading, loadData])

  // Re-execute on time range change
  const prevTimeRangeRef = useRef<{ begin: string; end: string } | null>(null)
  useEffect(() => {
    if (!config || isLoading) return
    if (prevTimeRangeRef.current === null) {
      prevTimeRangeRef.current = { begin: apiTimeRange.begin, end: apiTimeRange.end }
      return
    }
    if (
      prevTimeRangeRef.current.begin !== apiTimeRange.begin ||
      prevTimeRangeRef.current.end !== apiTimeRange.end
    ) {
      prevTimeRangeRef.current = { begin: apiTimeRange.begin, end: apiTimeRange.end }
      loadData(currentSqlRef.current)
    }
  }, [apiTimeRange, config, isLoading, loadData])

  const handleRunQuery = useCallback(
    (sql: string) => {
      loadData(sql)
    },
    [loadData]
  )

  const handleResetQuery = useCallback(() => {
    if (screen) {
      loadData(screen.config.sql)
      setHasUnsavedChanges(false)
    } else if (config) {
      loadData(config.sql)
    }
  }, [screen, config, loadData])

  const handleRefresh = useCallback(() => {
    loadData(currentSqlRef.current)
  }, [loadData])

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

  // Query editor panel
  const sqlPanel = config && screenType ? (
    <div className="flex flex-col h-full">
      <QueryEditor
        defaultSql={screen?.config.sql ?? config.sql}
        variables={VARIABLES}
        currentValues={currentValues}
        timeRangeLabel={timeRange.label}
        onRun={handleRunQuery}
        onReset={handleResetQuery}
        isLoading={streamQuery.isStreaming}
        error={queryError}
      />

      {/* Save buttons */}
      <div className="border-t border-theme-border p-3 flex gap-2">
        {screen && (
          <Button
            variant="default"
            size="sm"
            onClick={handleSave}
            disabled={isSaving || !hasUnsavedChanges}
            className="gap-1"
          >
            <Save className="w-4 h-4" />
            {isSaving ? 'Saving...' : 'Save'}
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
      </div>

      {saveError && (
        <div className="px-3 pb-3">
          <p className="text-xs text-accent-error">{saveError}</p>
        </div>
      )}
    </div>
  ) : undefined

  // Loading state
  if (isLoading) {
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

  return (
    <>
      <PageLayout onRefresh={handleRefresh} rightPanel={sqlPanel}>
        <div className="p-6 flex flex-col h-full">
          {/* Header */}
          <div className="mb-5">
            <div className="flex items-center gap-3">
              <div className="p-2 rounded-md bg-app-card text-accent-link">
                {getScreenTypeIcon(screenType)}
              </div>
              <div>
                <h1 className="text-2xl font-semibold text-theme-text-primary">
                  {isNew ? `New ${getScreenTypeDisplayName(screenType)} Screen` : screen?.name}
                </h1>
                <p className="text-sm text-theme-text-secondary">
                  {getScreenTypeDisplayName(screenType)}
                  {hasUnsavedChanges && (
                    <span className="ml-2 text-accent-warning">(unsaved changes)</span>
                  )}
                </p>
              </div>
            </div>
          </div>

          {/* Query Error */}
          {queryError && (
            <ErrorBanner
              title="Query execution failed"
              message={queryError}
              onRetry={streamQuery.error?.retryable ? handleRefresh : undefined}
            />
          )}

          {/* Results Table */}
          {streamQuery.isStreaming && !table ? (
            <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading data...</span>
              </div>
            </div>
          ) : table && table.numRows > 0 ? (
            <div className="flex-1 overflow-auto bg-app-panel border border-theme-border rounded-lg">
              <table className="w-full">
                <thead className="sticky top-0">
                  <tr className="bg-app-card border-b border-theme-border">
                    {table.schema.fields.map((field) => (
                      <th
                        key={field.name}
                        className="px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider text-theme-text-muted"
                      >
                        {field.name}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {Array.from({ length: table.numRows }, (_, i) => {
                    const row = table.get(i)
                    if (!row) return null
                    return (
                      <tr
                        key={i}
                        className="border-b border-theme-border hover:bg-app-card transition-colors"
                      >
                        {table.schema.fields.map((field) => {
                          const value = row[field.name]
                          const displayValue =
                            value === null || value === undefined
                              ? ''
                              : typeof value === 'object'
                                ? JSON.stringify(value)
                                : String(value)
                          return (
                            <td
                              key={field.name}
                              className="px-4 py-3 text-sm text-theme-text-primary"
                            >
                              {displayValue}
                            </td>
                          )
                        })}
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>
          ) : (
            <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
              <span className="text-theme-text-muted">No results</span>
            </div>
          )}

          {/* Row count */}
          {table && (
            <div className="mt-2 text-xs text-theme-text-muted text-center">
              {table.numRows} row{table.numRows !== 1 ? 's' : ''}
            </div>
          )}
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
