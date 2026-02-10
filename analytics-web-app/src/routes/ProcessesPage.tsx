import { Suspense, useState, useMemo, useCallback, useEffect, useRef } from 'react'
import { AppLink } from '@/components/AppLink'
import { ChevronUp, ChevronDown } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { PROCESSES_SCHEMA_URL } from '@/components/DocumentationLink'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useDefaultDataSource } from '@/hooks/useDefaultDataSource'
import { useScreenConfig } from '@/hooks/useScreenConfig'
import { useDebounce } from '@/hooks/useDebounce'
import { usePageTitle } from '@/hooks/usePageTitle'
import { formatTimestamp, formatDuration, parseTimeRange, getTimeRangeForApi } from '@/lib/time-range'
import { timestampToDate } from '@/lib/arrow-utils'
import type { ProcessesConfig } from '@/lib/screen-config'

type SortField = 'exe' | 'start_time' | 'last_update_time' | 'runtime' | 'username' | 'computer'
type SortDirection = 'asc' | 'desc'

const DEFAULT_SQL = `SELECT process_id, start_time, last_update_time, exe, computer, username
FROM processes
WHERE 1=1
  $search_filter
ORDER BY $order_by
LIMIT 100`

const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
  { name: 'order_by', description: 'Sort column and direction' },
  { name: 'search_filter', description: 'Expanded from search input' },
]

// Default config for ProcessesPage
const DEFAULT_CONFIG: ProcessesConfig = {
  timeRangeFrom: 'now-1h',
  timeRangeTo: 'now',
  search: '',
  sortField: 'last_update_time',
  sortDirection: 'desc',
}

// URL builder for ProcessesPage - builds query string from config
const buildUrl = (cfg: ProcessesConfig): string => {
  const params = new URLSearchParams()
  if (cfg.timeRangeFrom && cfg.timeRangeFrom !== DEFAULT_CONFIG.timeRangeFrom) {
    params.set('from', cfg.timeRangeFrom)
  }
  if (cfg.timeRangeTo && cfg.timeRangeTo !== DEFAULT_CONFIG.timeRangeTo) {
    params.set('to', cfg.timeRangeTo)
  }
  if (cfg.search) params.set('search', cfg.search)
  if (cfg.sortField && cfg.sortField !== DEFAULT_CONFIG.sortField) {
    params.set('sort', cfg.sortField)
  }
  if (cfg.sortDirection && cfg.sortDirection !== DEFAULT_CONFIG.sortDirection) {
    params.set('dir', cfg.sortDirection)
  }
  const qs = params.toString()
  return qs ? `?${qs}` : ''
}

// Expand search string into SQL ILIKE clauses for multi-word search.
// Note: These queries execute against DataFusion, a read-only analytics engine
// over our data lake. There are no INSERT/UPDATE/DELETE operations possible,
// so SQL injection risk is limited to information disclosure (mitigated by auth)
// and expensive queries (mitigated by timeouts).
function expandSearchFilter(search: string): string {
  const words = search.trim().split(/\s+/).filter(w => w.length > 0)
  if (words.length === 0) {
    return ''
  }

  const clauses = words.map(word => {
    // Escape SQL special characters for LIKE patterns
    const escaped = word
      .replace(/\\/g, '\\\\')
      .replace(/%/g, '\\%')
      .replace(/_/g, '\\_')
      .replace(/'/g, "''")
    return `(exe ILIKE '%${escaped}%' OR computer ILIKE '%${escaped}%' OR username ILIKE '%${escaped}%')`
  })

  return `AND ${clauses.join(' AND ')}`
}

function ProcessesPageContent() {
  usePageTitle('Processes')

  // Use the new config-driven pattern
  const { config, updateConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl)

  // Local state for sort (not persisted to URL by default - using replace)
  const sortField = (config.sortField ?? 'last_update_time') as SortField
  const sortDirection = (config.sortDirection ?? 'desc') as SortDirection

  // Local UI state for search input (debounced before syncing to config)
  const [searchInput, setSearchInput] = useState(config.search ?? '')
  const debouncedSearchInput = useDebounce(searchInput, 300)
  const [currentSql, setCurrentSql] = useState<string>(DEFAULT_SQL)

  // Compute API time range from config
  const apiTimeRange = useMemo(() => {
    try {
      return getTimeRangeForApi(config.timeRangeFrom ?? 'now-1h', config.timeRangeTo ?? 'now')
    } catch {
      return getTimeRangeForApi('now-1h', 'now')
    }
  }, [config.timeRangeFrom, config.timeRangeTo])

  // Compute display label for time range
  const timeRangeLabel = useMemo(() => {
    try {
      return parseTimeRange(config.timeRangeFrom ?? 'now-1h', config.timeRangeTo ?? 'now').label
    } catch {
      return 'Last 1 hour'
    }
  }, [config.timeRangeFrom, config.timeRangeTo])

  const defaultDataSource = useDefaultDataSource()
  const streamQuery = useStreamQuery()
  const table = streamQuery.getTable()
  const queryError = streamQuery.error?.message ?? null

  const currentSqlRef = useRef(currentSql)
  currentSqlRef.current = currentSql

  // Use ref to get latest execute function without causing re-renders
  const executeRef = useRef(streamQuery.execute)
  executeRef.current = streamQuery.execute

  const loadData = useCallback(
    (sql: string) => {
      setCurrentSql(sql)
      // Interpolate search_filter directly into SQL (it contains raw SQL with quotes)
      const sqlWithSearch = sql.replace('$search_filter', expandSearchFilter(config.search ?? ''))
      // Runtime is a computed column, so we need to use the SQL expression
      const orderByColumn = sortField === 'runtime'
        ? '(last_update_time - start_time)'
        : sortField
      const params: Record<string, string> = {
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
        order_by: `${orderByColumn} ${sortDirection.toUpperCase()}`,
      }
      executeRef.current({
        sql: sqlWithSearch,
        params,
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
        dataSource: defaultDataSource,
      })
    },
    [sortField, sortDirection, config.search, apiTimeRange, defaultDataSource]
  )

  // Sync debounced search input to config with replace (editing, not navigational)
  const isInitialSearchRef = useRef(true)
  useEffect(() => {
    if (isInitialSearchRef.current) {
      isInitialSearchRef.current = false
      return
    }
    updateConfig({ search: debouncedSearchInput.trim() || undefined }, { replace: true })
  }, [debouncedSearchInput, updateConfig])

  // Load on mount and when time range, sort, or search changes
  const queryKey = `${apiTimeRange.begin}-${apiTimeRange.end}-${sortField}-${sortDirection}-${config.search ?? ''}-${defaultDataSource}`
  const prevQueryKeyRef = useRef<string | null>(null)
  useEffect(() => {
    if (!defaultDataSource) return
    if (prevQueryKeyRef.current !== queryKey) {
      const isInitialLoad = prevQueryKeyRef.current === null
      prevQueryKeyRef.current = queryKey
      loadData(isInitialLoad ? DEFAULT_SQL : currentSqlRef.current)
    }
  }, [queryKey, loadData, defaultDataSource])

  // Time range changes create history entries (navigational)
  const handleTimeRangeChange = useCallback(
    (from: string, to: string) => {
      updateConfig({ timeRangeFrom: from, timeRangeTo: to })
    },
    [updateConfig]
  )

  const handleSort = (field: SortField) => {
    if (sortField === field) {
      updateConfig({ sortDirection: sortDirection === 'asc' ? 'desc' : 'asc' }, { replace: true })
    } else {
      updateConfig({ sortField: field, sortDirection: 'desc' }, { replace: true })
    }
  }

  const handleRunQuery = useCallback(
    (sql: string) => {
      loadData(sql)
    },
    [loadData]
  )

  const handleResetQuery = useCallback(() => {
    loadData(DEFAULT_SQL)
  }, [loadData])

  const currentValues = useMemo(
    () => {
      const orderByColumn = sortField === 'runtime'
        ? '(last_update_time - start_time)'
        : sortField
      return {
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
        order_by: `${orderByColumn} ${sortDirection.toUpperCase()}`,
        search_filter: expandSearchFilter(config.search ?? '') || '(empty)',
      }
    },
    [apiTimeRange, config.search, sortField, sortDirection]
  )

  const SortHeader = ({
    field,
    children,
    className = '',
  }: {
    field: SortField
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

  const sqlPanel = (
    <QueryEditor
      defaultSql={DEFAULT_SQL}
      variables={VARIABLES}
      currentValues={currentValues}
      timeRangeLabel={timeRangeLabel}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      isLoading={streamQuery.isStreaming}
      error={queryError}
      docLink={{
        url: PROCESSES_SCHEMA_URL,
        label: 'processes schema reference',
      }}
    />
  )

  const handleRefresh = useCallback(() => {
    loadData(currentSqlRef.current)
  }, [loadData])

  return (
    <AuthGuard>
      <PageLayout
        onRefresh={handleRefresh}
        rightPanel={sqlPanel}
        timeRangeControl={{
          timeRangeFrom: config.timeRangeFrom ?? 'now-1h',
          timeRangeTo: config.timeRangeTo ?? 'now',
          onTimeRangeChange: handleTimeRangeChange,
        }}
      >
        <div className="p-6 flex flex-col h-full">
          {/* Page Header */}
          <div className="mb-5">
            <h1 className="text-2xl font-semibold text-theme-text-primary">Processes</h1>
          </div>

          {/* Search */}
          <div className="mb-4">
            <input
              type="text"
              placeholder="Search by exe, computer, username..."
              value={searchInput}
              onChange={(e) => setSearchInput(e.target.value)}
              className="w-full max-w-md px-4 py-2.5 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-sm placeholder-theme-text-muted focus:outline-none focus:border-accent-link transition-colors"
            />
          </div>

          {/* Query Error Banner */}
          {queryError && (
            <ErrorBanner
              title="Query execution failed"
              message={queryError}
              onRetry={streamQuery.error?.retryable ? handleRefresh : undefined}
            />
          )}

          {/* Table */}
          {streamQuery.isStreaming && !table ? (
            <div className="flex-1 flex items-center justify-center bg-app-panel border border-theme-border rounded-lg">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading processes...</span>
              </div>
            </div>
          ) : (
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
                  {table && Array.from({ length: table.numRows }, (_, i) => {
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
                        key={processId}
                        className="border-b border-theme-border hover:bg-app-card transition-colors"
                      >
                        <td className="px-4 py-3">
                          <AppLink
                            href={`/process?process_id=${processId}&from=${encodeURIComponent(fromParam)}&to=${encodeURIComponent(toParam)}`}
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
                  {(!table || table.numRows === 0) && (
                    <tr>
                      <td colSpan={7} className="px-4 py-8 text-center text-theme-text-muted">
                        {config.search ? 'No processes match your search.' : 'No processes available.'}
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </PageLayout>
    </AuthGuard>
  )
}

export default function ProcessesPage() {
  return (
    <Suspense
      fallback={
        <AuthGuard>
          <PageLayout>
            <div className="p-6">
              <div className="flex items-center justify-center h-64">
                <div className="animate-spin rounded-full h-8 w-8 border-2 border-accent-link border-t-transparent" />
              </div>
            </div>
          </PageLayout>
        </AuthGuard>
      }
    >
      <ProcessesPageContent />
    </Suspense>
  )
}
