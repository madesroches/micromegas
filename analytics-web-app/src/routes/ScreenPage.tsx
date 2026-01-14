import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useParams, useSearchParams, useNavigate } from 'react-router-dom'
import { AlertCircle, Save, ChevronUp, ChevronDown } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { ErrorBanner } from '@/components/ErrorBanner'
import { QueryEditor } from '@/components/QueryEditor'
import { TimeSeriesChart, type ScaleMode } from '@/components/TimeSeriesChart'
import { Button } from '@/components/ui/button'
import { AppLink } from '@/components/AppLink'
import { SaveScreenDialog } from '@/components/SaveScreenDialog'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useTimeRange } from '@/hooks/useTimeRange'
import { formatTimestamp, formatDuration } from '@/lib/time-range'
import { timestampToDate, timestampToMs } from '@/lib/arrow-utils'
import { renderIcon } from '@/lib/screen-type-utils'
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

// Variables available for all screen types
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
]

// Process list sorting
type ProcessSortField = 'exe' | 'start_time' | 'last_update_time' | 'runtime' | 'username' | 'computer'
type SortDirection = 'asc' | 'desc'

// Process list table component (matches ProcessesPage)
function ProcessListTable({
  table,
  sortField,
  sortDirection,
  onSort,
}: {
  table: ReturnType<ReturnType<typeof useStreamQuery>['getTable']>
  sortField: ProcessSortField
  sortDirection: SortDirection
  onSort: (field: ProcessSortField) => void
}) {
  const SortHeader = ({
    field,
    children,
    className = '',
  }: {
    field: ProcessSortField
    children: React.ReactNode
    className?: string
  }) => (
    <th
      onClick={() => onSort(field)}
      className={`px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider cursor-pointer select-none transition-colors ${
        sortField === field
          ? 'text-theme-text-primary bg-app-card'
          : 'text-theme-text-muted hover:text-theme-text-secondary hover:bg-app-card'
      } ${className}`}
    >
      <div className="flex items-center gap-1">
        {children}
        <span className={sortField === field ? 'text-accent-link' : 'opacity-30'}>
          {sortField === field && sortDirection === 'asc' ? (
            <ChevronUp className="w-3 h-3" />
          ) : (
            <ChevronDown className="w-3 h-3" />
          )}
        </span>
      </div>
    </th>
  )

  if (!table || table.numRows === 0) {
    return (
      <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
        <span className="text-theme-text-muted">No processes available.</span>
      </div>
    )
  }

  return (
    <div className="flex-1 overflow-auto bg-app-panel border border-theme-border rounded-lg">
      <table className="w-full">
        <thead className="sticky top-0">
          <tr className="bg-app-card border-b border-theme-border">
            <SortHeader field="exe">Process</SortHeader>
            <th className="hidden sm:table-cell px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider text-theme-text-muted">
              Process ID
            </th>
            <SortHeader field="start_time">Start Time</SortHeader>
            <SortHeader field="last_update_time" className="hidden lg:table-cell">
              Last Update
            </SortHeader>
            <SortHeader field="runtime" className="hidden lg:table-cell">
              Runtime
            </SortHeader>
            <SortHeader field="username" className="hidden md:table-cell">
              Username
            </SortHeader>
            <SortHeader field="computer" className="hidden md:table-cell">
              Computer
            </SortHeader>
          </tr>
        </thead>
        <tbody>
          {Array.from({ length: table.numRows }, (_, i) => {
            const row = table.get(i)
            if (!row) return null
            const processId = String(row.process_id ?? '')
            const exe = String(row.exe ?? '')
            const startTime = row.start_time
            const lastUpdateTime = row.last_update_time
            const username = String(row.username ?? '')
            const computer = String(row.computer ?? '')
            const startDate = timestampToDate(startTime)
            const endDate = timestampToDate(lastUpdateTime)
            const fromParam = startDate?.toISOString() ?? ''
            const toParam = endDate?.toISOString() ?? ''
            return (
              <tr
                key={processId || i}
                className="border-b border-theme-border hover:bg-app-card transition-colors"
              >
                <td className="px-4 py-3">
                  <AppLink
                    href={`/process?id=${processId}&from=${encodeURIComponent(fromParam)}&to=${encodeURIComponent(toParam)}`}
                    className="text-accent-link hover:underline"
                  >
                    {exe}
                  </AppLink>
                </td>
                <td className="hidden sm:table-cell px-4 py-3">
                  <CopyableProcessId
                    processId={processId}
                    truncate={true}
                    className="text-sm font-mono text-theme-text-secondary"
                  />
                </td>
                <td className="px-4 py-3 font-mono text-sm text-theme-text-primary">
                  {formatTimestamp(startTime)}
                </td>
                <td className="hidden lg:table-cell px-4 py-3 font-mono text-sm text-theme-text-primary">
                  {formatTimestamp(lastUpdateTime)}
                </td>
                <td className="hidden lg:table-cell px-4 py-3 font-mono text-sm text-theme-text-secondary">
                  {formatDuration(startTime, lastUpdateTime)}
                </td>
                <td className="hidden md:table-cell px-4 py-3 text-theme-text-primary">
                  {username}
                </td>
                <td className="hidden md:table-cell px-4 py-3 text-theme-text-primary">
                  {computer}
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

// Metrics chart component for metrics screen type
function MetricsView({
  table,
  onTimeRangeSelect,
  scaleMode,
  onScaleModeChange,
}: {
  table: ReturnType<ReturnType<typeof useStreamQuery>['getTable']>
  onTimeRangeSelect?: (from: Date, to: Date) => void
  scaleMode: ScaleMode
  onScaleModeChange: (mode: ScaleMode) => void
}) {
  // Transform table data to chart format
  const chartData = useMemo(() => {
    if (!table || table.numRows === 0) return []
    const points: { time: number; value: number }[] = []

    for (let i = 0; i < table.numRows; i++) {
      const row = table.get(i)
      if (row) {
        const time = timestampToMs(row.time)
        const value = Number(row.value)
        if (!isNaN(time) && !isNaN(value)) {
          points.push({ time, value })
        }
      }
    }
    // Sort by time ascending - uPlot requires data in chronological order
    points.sort((a, b) => a.time - b.time)
    return points
  }, [table])

  if (!table || table.numRows === 0) {
    return (
      <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
        <span className="text-theme-text-muted">No data available.</span>
      </div>
    )
  }

  if (chartData.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
        <span className="text-theme-text-muted">No valid time/value data found. Query must return &apos;time&apos; and &apos;value&apos; columns.</span>
      </div>
    )
  }

  return (
    <div className="flex-1 min-h-[400px] h-full">
      <TimeSeriesChart
        data={chartData}
        title=""
        unit=""
        scaleMode={scaleMode}
        onScaleModeChange={onScaleModeChange}
        onTimeRangeSelect={onTimeRangeSelect}
      />
    </div>
  )
}

function ScreenPageContent() {
  const { name } = useParams<{ name: string }>()
  const [searchParams] = useSearchParams()
  const navigate = useNavigate()
  const isNew = !name
  const typeParam = searchParams.get('type') as ScreenTypeName | null

  const { parsed: timeRange, apiTimeRange, setTimeRange } = useTimeRange()

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

  // Process list sorting state
  const [sortField, setSortField] = useState<ProcessSortField>('last_update_time')
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc')

  // Metrics chart state
  const [metricsScaleMode, setMetricsScaleMode] = useState<ScaleMode>('p99')

  // Screen type info (fetched from API)
  const [screenTypeInfo, setScreenTypeInfo] = useState<ScreenTypeInfo | null>(null)

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
          setConfig(defaultConfig)
          setScreenType(typeParam)
          setScreenTypeInfo(typeMap.get(typeParam) ?? null)
          currentSqlRef.current = defaultConfig.sql
        } else {
          // Existing screen - load from API
          const loadedScreen = await getScreen(name)
          setScreen(loadedScreen)
          setConfig(loadedScreen.config)
          setScreenType(loadedScreen.screen_type as ScreenTypeName)
          setScreenTypeInfo(typeMap.get(loadedScreen.screen_type as ScreenTypeName) ?? null)
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

  // Track SQL changes immediately as user types
  const handleSqlChange = useCallback(
    (sql: string) => {
      if (screen && sql !== screen.config.sql) {
        setHasUnsavedChanges(true)
      } else if (screen && sql === screen.config.sql) {
        setHasUnsavedChanges(false)
      }
    },
    [screen]
  )

  const handleRefresh = useCallback(() => {
    loadData(currentSqlRef.current)
  }, [loadData])

  // Handle process list sorting
  const handleSort = useCallback((field: ProcessSortField) => {
    if (sortField === field) {
      setSortDirection(sortDirection === 'asc' ? 'desc' : 'asc')
    } else {
      setSortField(field)
      setSortDirection('desc')
    }
  }, [sortField, sortDirection])

  // Sync metrics scale mode from config when loaded
  useEffect(() => {
    if (config?.metrics_options?.scale_mode) {
      setMetricsScaleMode(config.metrics_options.scale_mode)
    }
  }, [config?.metrics_options?.scale_mode])

  // Handle time range selection from chart drag
  const handleTimeRangeSelect = useCallback(
    (from: Date, to: Date) => {
      setTimeRange(from.toISOString(), to.toISOString())
    },
    [setTimeRange]
  )

  // Handle metrics scale mode change
  const handleScaleModeChange = useCallback(
    (mode: ScaleMode) => {
      setMetricsScaleMode(mode)
      setConfig((prev) =>
        prev
          ? {
              ...prev,
              metrics_options: { ...prev.metrics_options, scale_mode: mode },
            }
          : null
      )
      if (screen && screen.config.metrics_options?.scale_mode !== mode) {
        setHasUnsavedChanges(true)
      }
    },
    [screen]
  )

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
    <QueryEditor
      defaultSql={screen?.config.sql ?? config.sql}
      variables={VARIABLES}
      currentValues={currentValues}
      timeRangeLabel={timeRange.label}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      onChange={handleSqlChange}
      isLoading={streamQuery.isStreaming}
      error={queryError}
      footer={
        <>
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
        </>
      }
    />
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

          {/* Query Error */}
          {queryError && (
            <ErrorBanner
              title="Query execution failed"
              message={queryError}
              onRetry={streamQuery.error?.retryable ? handleRefresh : undefined}
            />
          )}

          {/* Results */}
          {streamQuery.isStreaming && !table ? (
            <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading data...</span>
              </div>
            </div>
          ) : screenType === 'process_list' ? (
            <ProcessListTable
              table={table}
              sortField={sortField}
              sortDirection={sortDirection}
              onSort={handleSort}
            />
          ) : screenType === 'metrics' ? (
            <MetricsView
              table={table}
              onTimeRangeSelect={handleTimeRangeSelect}
              scaleMode={metricsScaleMode}
              onScaleModeChange={handleScaleModeChange}
            />
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
