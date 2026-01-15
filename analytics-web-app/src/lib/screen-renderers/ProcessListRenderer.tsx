import { useState, useCallback } from 'react'
import { ChevronUp, ChevronDown } from 'lucide-react'
import { registerRenderer, ScreenRendererProps } from './index'
import { useScreenQuery } from './useScreenQuery'
import { useTimeRangeSync } from './useTimeRangeSync'
import { LoadingState, EmptyState, SaveFooter, RendererLayout } from './shared'
import { QueryEditor } from '@/components/QueryEditor'
import { AppLink } from '@/components/AppLink'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { formatTimestamp, formatDuration } from '@/lib/time-range'
import { timestampToDate } from '@/lib/arrow-utils'

// Variables available for process list queries
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
]

// Sorting types
type ProcessSortField = 'exe' | 'start_time' | 'last_update_time' | 'runtime' | 'username' | 'computer'
type SortDirection = 'asc' | 'desc'

interface ProcessListConfig {
  sql: string
  timeRangeFrom?: string
  timeRangeTo?: string
}

export function ProcessListRenderer({
  config,
  onConfigChange,
  savedConfig,
  onUnsavedChange,
  timeRange,
  rawTimeRange,
  timeRangeLabel,
  currentValues,
  onSave,
  isSaving,
  hasUnsavedChanges,
  onSaveAs,
  saveError,
  refreshTrigger,
}: ScreenRendererProps) {
  const processListConfig = config as ProcessListConfig
  const savedProcessListConfig = savedConfig as ProcessListConfig | null

  // Query execution
  const query = useScreenQuery({
    initialSql: processListConfig.sql,
    timeRange,
    refreshTrigger,
  })

  // Sorting state (UI-only, not persisted)
  const [sortField, setSortField] = useState<ProcessSortField>('last_update_time')
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc')

  // Sync time range changes to config
  useTimeRangeSync({
    rawTimeRange,
    savedConfig: savedProcessListConfig,
    config: processListConfig,
    onUnsavedChange,
    onConfigChange,
  })

  const handleRunQuery = useCallback(
    (sql: string) => {
      onConfigChange({ ...processListConfig, sql })
      if (savedConfig && sql !== (savedConfig as ProcessListConfig).sql) {
        onUnsavedChange()
      }
      query.execute(sql)
    },
    [processListConfig, savedConfig, onConfigChange, onUnsavedChange, query]
  )

  const handleResetQuery = useCallback(() => {
    const sql = savedConfig ? (savedConfig as ProcessListConfig).sql : processListConfig.sql
    handleRunQuery(sql)
  }, [savedConfig, processListConfig.sql, handleRunQuery])

  const handleSqlChange = useCallback(
    (sql: string) => {
      if (savedConfig && sql !== (savedConfig as ProcessListConfig).sql) {
        onUnsavedChange()
      }
    },
    [savedConfig, onUnsavedChange]
  )

  const handleSort = useCallback(
    (field: ProcessSortField) => {
      if (sortField === field) {
        setSortDirection(sortDirection === 'asc' ? 'desc' : 'asc')
      } else {
        setSortField(field)
        setSortDirection('desc')
      }
    },
    [sortField, sortDirection]
  )

  // Sort header component
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
      onClick={() => handleSort(field)}
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

  // Query editor panel
  const sqlPanel = (
    <QueryEditor
      defaultSql={savedConfig ? (savedConfig as ProcessListConfig).sql : processListConfig.sql}
      variables={VARIABLES}
      currentValues={currentValues}
      timeRangeLabel={timeRangeLabel}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      onChange={handleSqlChange}
      isLoading={query.isLoading}
      error={query.error}
      footer={
        <SaveFooter
          onSave={onSave}
          onSaveAs={onSaveAs}
          isSaving={isSaving}
          hasUnsavedChanges={hasUnsavedChanges}
          saveError={saveError}
        />
      }
    />
  )

  // Render content
  const renderContent = () => {
    const table = query.table

    if (query.isLoading && !table) {
      return <LoadingState message="Loading data..." />
    }

    if (!table || table.numRows === 0) {
      return <EmptyState message="No processes available." />
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

  return (
    <RendererLayout
      error={query.error}
      isRetryable={query.isRetryable}
      onRetry={query.retry}
      sqlPanel={sqlPanel}
    >
      {renderContent()}
    </RendererLayout>
  )
}

// Register this renderer
registerRenderer('process_list', ProcessListRenderer)
