import { Suspense, useState, useMemo, useCallback, useEffect, useRef } from 'react'
import { useSearchParams, useNavigate, useLocation } from 'react-router-dom'
import { useMutation } from '@tanstack/react-query'
import { AppLink } from '@/components/AppLink'
import { ChevronUp, ChevronDown } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { executeSqlQuery, toRowObjects } from '@/lib/api'
import { useTimeRange } from '@/hooks/useTimeRange'
import { useDebounce } from '@/hooks/useDebounce'
import { formatTimestamp, formatDuration } from '@/lib/time-range'
import { SqlRow } from '@/types'

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
  const [searchParams] = useSearchParams()
  const navigate = useNavigate()
  const location = useLocation()
  const pathname = location.pathname

  // Read initial search from URL
  const initialSearch = searchParams.get('search') || ''

  const [searchInput, setSearchInput] = useState(initialSearch)
  const [search, setSearch] = useState(initialSearch)
  const debouncedSearchInput = useDebounce(searchInput, 300)
  const [sortField, setSortField] = useState<SortField>('last_update_time')
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc')
  const [queryError, setQueryError] = useState<string | null>(null)
  const [rows, setRows] = useState<SqlRow[]>([])
  const [currentSql, setCurrentSql] = useState<string>(DEFAULT_SQL)
  const { parsed: timeRange, apiTimeRange } = useTimeRange()

  const sqlMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      setQueryError(null)
      setRows(toRowObjects(data))
    },
    onError: (err: Error) => {
      setQueryError(err.message)
    },
  })

  // Load data - using ref to avoid including mutation in deps
  const mutateRef = useRef(sqlMutation.mutate)
  mutateRef.current = sqlMutation.mutate

  const currentSqlRef = useRef(currentSql)
  currentSqlRef.current = currentSql

  const loadData = useCallback(
    (sql: string) => {
      setQueryError(null)
      setCurrentSql(sql)
      // Interpolate search_filter directly into SQL (it contains raw SQL with quotes)
      const sqlWithSearch = sql.replace('$search_filter', expandSearchFilter(search))
      // Runtime is a computed column, so we need to use the SQL expression
      const orderByColumn = sortField === 'runtime'
        ? '(last_update_time - start_time)'
        : sortField
      const params: Record<string, string> = {
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
        order_by: `${orderByColumn} ${sortDirection.toUpperCase()}`,
      }
      mutateRef.current({
        sql: sqlWithSearch,
        params,
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    },
    [sortField, sortDirection, search, apiTimeRange]
  )

  // Update search state and URL
  const updateSearch = useCallback(
    (value: string) => {
      setSearch(value)
      const params = new URLSearchParams(searchParams.toString())
      if (value.trim() === '') {
        params.delete('search')
      } else {
        params.set('search', value.trim())
      }
      navigate(`${pathname}?${params.toString()}`, { replace: true })
    },
    [searchParams, navigate, pathname]
  )

  // Sync debounced input to search state and URL
  const isInitialSearchRef = useRef(true)
  useEffect(() => {
    if (isInitialSearchRef.current) {
      isInitialSearchRef.current = false
      return
    }
    updateSearch(debouncedSearchInput)
  }, [debouncedSearchInput, updateSearch])

  // Load on mount and when time range, sort, or search changes
  const queryKey = `${apiTimeRange.begin}-${apiTimeRange.end}-${sortField}-${sortDirection}-${search}`
  const prevQueryKeyRef = useRef<string | null>(null)
  useEffect(() => {
    if (prevQueryKeyRef.current !== queryKey) {
      const isInitialLoad = prevQueryKeyRef.current === null
      prevQueryKeyRef.current = queryKey
      loadData(isInitialLoad ? DEFAULT_SQL : currentSqlRef.current)
    }
  }, [queryKey, loadData])

  const handleSort = (field: SortField) => {
    if (sortField === field) {
      setSortDirection(sortDirection === 'asc' ? 'desc' : 'asc')
    } else {
      setSortField(field)
      setSortDirection('desc')
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
        search_filter: expandSearchFilter(search) || '(empty)',
      }
    },
    [apiTimeRange, search, sortField, sortDirection]
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
      timeRangeLabel={timeRange.label}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      isLoading={sqlMutation.isPending}
      error={queryError}
      docLink={{
        url: 'https://madesroches.github.io/micromegas/docs/query-guide/schema-reference/#processes',
        label: 'processes schema reference',
      }}
    />
  )

  const handleRefresh = useCallback(() => {
    loadData(currentSqlRef.current)
  }, [loadData])

  return (
    <AuthGuard>
      <PageLayout onRefresh={handleRefresh} rightPanel={sqlPanel}>
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
              onDismiss={() => setQueryError(null)}
              onRetry={handleRefresh}
            />
          )}

          {/* Table */}
          {sqlMutation.isPending && rows.length === 0 ? (
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
                  {rows.map((row) => (
                    <tr
                      key={String(row.process_id)}
                      className="border-b border-theme-border hover:bg-app-card transition-colors"
                    >
                      <td className="px-4 py-3">
                        <AppLink
                          href={`/process?id=${row.process_id}&from=${encodeURIComponent(String(row.start_time))}&to=${encodeURIComponent(String(row.last_update_time))}`}
                          className="text-accent-link hover:underline"
                        >
                          {String(row.exe ?? '')}
                        </AppLink>
                      </td>
                      <td className="hidden sm:table-cell px-4 py-3">
                        <CopyableProcessId
                          processId={String(row.process_id ?? '')}
                          truncate={true}
                          className="text-sm font-mono text-theme-text-secondary"
                        />
                      </td>
                      <td className="px-4 py-3 font-mono text-sm text-theme-text-primary">
                        {formatTimestamp(row.start_time)}
                      </td>
                      <td className="hidden lg:table-cell px-4 py-3 font-mono text-sm text-theme-text-primary">
                        {formatTimestamp(row.last_update_time)}
                      </td>
                      <td className="hidden lg:table-cell px-4 py-3 font-mono text-sm text-theme-text-secondary">
                        {formatDuration(row.start_time, row.last_update_time)}
                      </td>
                      <td className="hidden md:table-cell px-4 py-3 text-theme-text-primary">
                        {String(row.username ?? '')}
                      </td>
                      <td className="hidden md:table-cell px-4 py-3 text-theme-text-primary">
                        {String(row.computer ?? '')}
                      </td>
                    </tr>
                  ))}
                  {rows.length === 0 && (
                    <tr>
                      <td colSpan={7} className="px-4 py-8 text-center text-theme-text-muted">
                        {search ? 'No processes match your search.' : 'No processes available.'}
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
