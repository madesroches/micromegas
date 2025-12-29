import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useSearchParams, useNavigate, useLocation } from 'react-router-dom'
import { AppLink } from '@/components/AppLink'
import { AlertCircle, ChevronDown } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useTimeRange } from '@/hooks/useTimeRange'
import { useDebounce } from '@/hooks/useDebounce'

const DEFAULT_SQL = `SELECT time, level, target, msg
FROM view_instance('log_entries', '$process_id')
WHERE level <= $max_level
  $search_filter
ORDER BY time DESC
LIMIT $limit`

const VARIABLES = [
  { name: 'process_id', description: 'Current process ID' },
  { name: 'max_level', description: 'Max log level filter (1-6)' },
  { name: 'limit', description: 'Row limit' },
  { name: 'search_filter', description: 'Expanded from search input' },
]

const LOG_LEVELS: Record<string, number> = {
  all: 6,
  trace: 6,
  debug: 5,
  info: 4,
  warn: 3,
  error: 2,
  fatal: 1,
}

const LEVEL_NAMES: Record<number, string> = {
  1: 'FATAL',
  2: 'ERROR',
  3: 'WARN',
  4: 'INFO',
  5: 'DEBUG',
  6: 'TRACE',
}

const VALID_LEVELS = ['all', 'trace', 'debug', 'info', 'warn', 'error', 'fatal']
const PRESET_LIMITS = [50, 100, 200, 500, 1000]
const MIN_LIMIT = 1
const MAX_LIMIT = 10000

function parseLimit(value: string | null): number {
  if (!value) return 100
  const parsed = parseInt(value, 10)
  if (isNaN(parsed) || parsed < MIN_LIMIT) return 100
  return Math.min(parsed, MAX_LIMIT)
}

function expandSearchFilter(search: string): string {
  const words = search.trim().split(/\s+/).filter(w => w.length > 0)
  if (words.length === 0) {
    return ''
  }

  const clauses = words.map(word => {
    const escaped = word
      .replace(/\\/g, '\\\\')
      .replace(/%/g, '\\%')
      .replace(/_/g, '\\_')
      .replace(/'/g, "''")
    return `(target ILIKE '%${escaped}%' OR msg ILIKE '%${escaped}%')`
  })

  return `AND ${clauses.join(' AND ')}`
}

interface EditableComboboxProps {
  value: string
  options: number[]
  onChange: (value: string) => void
  onSelect: (value: number) => void
  onBlur: () => void
  onKeyDown: (e: React.KeyboardEvent<HTMLInputElement>) => void
  className?: string
}

function EditableCombobox({ value, options, onChange, onSelect, onBlur, onKeyDown, className }: EditableComboboxProps) {
  const [isOpen, setIsOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setIsOpen(false)
      }
    }
    document.addEventListener('mousedown', handleClickOutside)
    return () => document.removeEventListener('mousedown', handleClickOutside)
  }, [])

  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const filtered = e.target.value.replace(/[^0-9]/g, '')
    onChange(filtered)
  }

  return (
    <div ref={containerRef} className={`relative ${className || ''}`}>
      <div className="flex">
        <input
          type="text"
          inputMode="numeric"
          value={value}
          onChange={handleInputChange}
          onBlur={onBlur}
          onKeyDown={onKeyDown}
          className="w-20 px-3 py-2 bg-app-panel border border-theme-border rounded-l-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
        />
        <button
          type="button"
          onClick={() => setIsOpen(!isOpen)}
          aria-expanded={isOpen}
          aria-label="Select preset limit"
          tabIndex={-1}
          className="px-2 py-2 bg-app-panel border border-l-0 border-theme-border rounded-r-md text-theme-text-secondary hover:bg-theme-bg-hover focus:outline-none focus:border-accent-link"
        >
          <ChevronDown className="w-4 h-4" />
        </button>
      </div>
      {isOpen && (
        <div className="absolute top-full left-0 mt-1 w-full bg-app-panel border border-theme-border rounded-md shadow-lg z-50">
          {options.map((option) => (
            <button
              key={option}
              type="button"
              onClick={() => {
                onSelect(option)
                setIsOpen(false)
              }}
              className="w-full px-3 py-2 text-left text-sm text-theme-text-primary hover:bg-theme-bg-hover first:rounded-t-md last:rounded-b-md"
            >
              {option}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

// Convert Arrow timestamp to Date
function arrowTimestampToDate(value: unknown): Date | null {
  if (!value) return null
  if (value instanceof Date) return value
  // Arrow timestamps can be numbers (ms) or BigInt (ns/us)
  if (typeof value === 'number') return new Date(value)
  if (typeof value === 'bigint') {
    // Assume microseconds, convert to milliseconds
    return new Date(Number(value / 1000n))
  }
  // Try parsing as string
  const date = new Date(String(value))
  return isNaN(date.getTime()) ? null : date
}

function formatLocalTime(utcTime: unknown): string {
  if (!utcTime) return ''.padEnd(29)

  const date = arrowTimestampToDate(utcTime)
  if (!date) return ''.padEnd(29)

  // Try to extract nanoseconds from string representation
  let nanoseconds = '000000000'
  const str = String(utcTime)
  const nanoMatch = str.match(/\.(\d+)/)
  if (nanoMatch) {
    nanoseconds = nanoMatch[1].padEnd(9, '0').slice(0, 9)
  }

  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  const seconds = String(date.getSeconds()).padStart(2, '0')

  return `${year}-${month}-${day} ${hours}:${minutes}:${seconds}.${nanoseconds}`
}

interface LogRow {
  time: unknown
  level: string
  target: string
  msg: string
}

function ProcessLogContent() {
  const [searchParams] = useSearchParams()
  const navigate = useNavigate()
  const location = useLocation()
  const pathname = location.pathname
  const processId = searchParams.get('process_id')
  const { parsed: timeRange, apiTimeRange } = useTimeRange()

  const levelParam = searchParams.get('level')
  const limitParam = searchParams.get('limit')
  const searchParam = searchParams.get('search')
  const initialLevel = levelParam && VALID_LEVELS.includes(levelParam) ? levelParam : 'all'
  const initialLimit = parseLimit(limitParam)
  const initialSearch = searchParam || ''

  const [logLevel, setLogLevel] = useState<string>(initialLevel)
  const [logLimit, setLogLimit] = useState<number>(initialLimit)
  const [limitInputValue, setLimitInputValue] = useState<string>(String(initialLimit))
  const [search, setSearch] = useState<string>(initialSearch)
  const [searchInputValue, setSearchInputValue] = useState<string>(initialSearch)
  const debouncedSearchInput = useDebounce(searchInputValue, 300)
  const [rows, setRows] = useState<LogRow[]>([])
  const [hasLoaded, setHasLoaded] = useState(false)
  const [currentSql, setCurrentSql] = useState<string>(DEFAULT_SQL)

  const streamQuery = useStreamQuery()
  const queryError = streamQuery.error?.message ?? null

  // Extract rows when query completes
  useEffect(() => {
    if (streamQuery.isComplete && !streamQuery.error) {
      const table = streamQuery.getTable()
      if (table) {
        const resultRows: LogRow[] = []
        for (let i = 0; i < table.numRows; i++) {
          const row = table.get(i)
          if (row) {
            const levelValue = row.level
            const levelStr = typeof levelValue === 'number'
              ? (LEVEL_NAMES[levelValue] || 'UNKNOWN')
              : String(levelValue ?? '')
            resultRows.push({
              time: row.time,
              level: levelStr,
              target: String(row.target ?? ''),
              msg: String(row.msg ?? ''),
            })
          }
        }
        setRows(resultRows)
        setHasLoaded(true)
      }
    }
  }, [streamQuery.isComplete, streamQuery.error])

  const executeRef = useRef(streamQuery.execute)
  executeRef.current = streamQuery.execute

  const currentSqlRef = useRef(currentSql)
  currentSqlRef.current = currentSql

  const loadData = useCallback(
    (sql: string) => {
      if (!processId) return
      setCurrentSql(sql)
      const sqlWithSearch = sql.replace('$search_filter', expandSearchFilter(search))
      const params: Record<string, string> = {
        process_id: processId,
        max_level: String(LOG_LEVELS[logLevel] || 6),
        limit: String(logLimit),
      }
      executeRef.current({
        sql: sqlWithSearch,
        params,
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    },
    [processId, logLevel, logLimit, search, apiTimeRange]
  )

  const updateLogLevel = useCallback(
    (level: string) => {
      setLogLevel(level)
      const params = new URLSearchParams(searchParams.toString())
      if (level === 'all') {
        params.delete('level')
      } else {
        params.set('level', level)
      }
      navigate(`${pathname}?${params.toString()}`, { replace: true })
    },
    [searchParams, navigate, pathname]
  )

  const updateLogLimit = useCallback(
    (limit: number) => {
      const clampedLimit = Math.max(MIN_LIMIT, Math.min(MAX_LIMIT, limit))
      setLogLimit(clampedLimit)
      setLimitInputValue(String(clampedLimit))
      const params = new URLSearchParams(searchParams.toString())
      if (clampedLimit === 100) {
        params.delete('limit')
      } else {
        params.set('limit', String(clampedLimit))
      }
      navigate(`${pathname}?${params.toString()}`, { replace: true })
    },
    [searchParams, navigate, pathname]
  )

  const handleLimitInputBlur = useCallback(() => {
    const parsed = parseInt(limitInputValue, 10)
    if (isNaN(parsed) || parsed < MIN_LIMIT) {
      setLimitInputValue(String(logLimit))
    } else {
      updateLogLimit(parsed)
    }
  }, [limitInputValue, logLimit, updateLogLimit])

  const handleLimitInputKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter') {
        e.currentTarget.blur()
      }
    },
    []
  )

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

  const isInitialSearchRef = useRef(true)
  useEffect(() => {
    if (isInitialSearchRef.current) {
      isInitialSearchRef.current = false
      return
    }
    updateSearch(debouncedSearchInput)
  }, [debouncedSearchInput, updateSearch])

  const handleSearchBlur = useCallback(() => {
    if (searchInputValue !== search) {
      updateSearch(searchInputValue)
    }
  }, [searchInputValue, search, updateSearch])

  const handleSearchKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter') {
        e.currentTarget.blur()
      }
    },
    []
  )

  const hasInitialLoadRef = useRef(false)
  useEffect(() => {
    if (processId && !hasInitialLoadRef.current) {
      hasInitialLoadRef.current = true
      loadData(DEFAULT_SQL)
    }
  }, [processId, loadData])

  const prevFiltersRef = useRef<{ logLevel: string; logLimit: number; search: string } | null>(null)
  useEffect(() => {
    if (!hasLoaded) return
    if (prevFiltersRef.current === null) {
      prevFiltersRef.current = { logLevel, logLimit, search }
      return
    }
    if (prevFiltersRef.current.logLevel !== logLevel || prevFiltersRef.current.logLimit !== logLimit || prevFiltersRef.current.search !== search) {
      prevFiltersRef.current = { logLevel, logLimit, search }
      loadData(currentSqlRef.current)
    }
  }, [logLevel, logLimit, search, hasLoaded, loadData])

  const prevTimeRangeRef = useRef<{ begin: string; end: string } | null>(null)
  useEffect(() => {
    if (!hasLoaded) return
    if (prevTimeRangeRef.current === null) {
      prevTimeRangeRef.current = { begin: apiTimeRange.begin, end: apiTimeRange.end }
      return
    }
    if (prevTimeRangeRef.current.begin !== apiTimeRange.begin || prevTimeRangeRef.current.end !== apiTimeRange.end) {
      prevTimeRangeRef.current = { begin: apiTimeRange.begin, end: apiTimeRange.end }
      loadData(currentSqlRef.current)
    }
  }, [apiTimeRange.begin, apiTimeRange.end, hasLoaded, loadData])

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
    () => ({
      process_id: processId || '',
      max_level: String(LOG_LEVELS[logLevel] || 6),
      limit: String(logLimit),
      search_filter: expandSearchFilter(search) || '(empty)',
    }),
    [processId, logLevel, logLimit, search]
  )

  const getLevelColor = (level: unknown) => {
    const levelStr = String(level)
    switch (levelStr) {
      case 'FATAL':
        return 'text-accent-error-bright'
      case 'ERROR':
        return 'text-accent-error'
      case 'WARN':
        return 'text-accent-warning'
      case 'INFO':
        return 'text-accent-link'
      case 'DEBUG':
        return 'text-theme-text-secondary'
      case 'TRACE':
        return 'text-theme-text-muted'
      default:
        return 'text-theme-text-primary'
    }
  }

  const sqlPanel = processId ? (
    <QueryEditor
      defaultSql={DEFAULT_SQL}
      variables={VARIABLES}
      currentValues={currentValues}
      timeRangeLabel={timeRange.label}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      isLoading={streamQuery.isStreaming}
      error={queryError}
      docLink={{
        url: 'https://madesroches.github.io/micromegas/docs/query-guide/schema-reference/#log_entries',
        label: 'log_entries schema reference',
      }}
    />
  ) : undefined

  const handleRefresh = useCallback(() => {
    loadData(currentSqlRef.current)
  }, [loadData])

  if (!processId) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-app-panel border border-theme-border rounded-lg">
            <AlertCircle className="w-10 h-10 text-accent-error mb-3" />
            <p className="text-theme-text-secondary">No process ID provided</p>
            <AppLink href="/processes" className="text-accent-link hover:underline mt-2">
              Back to Processes
            </AppLink>
          </div>
        </div>
      </PageLayout>
    )
  }

  return (
    <PageLayout onRefresh={handleRefresh} rightPanel={sqlPanel}>
      <div className="p-6 flex flex-col h-full">
        <div className="mb-5">
          <h1 className="text-2xl font-semibold text-theme-text-primary">Process Log</h1>
          <div className="text-sm text-theme-text-muted font-mono mt-1">
            <CopyableProcessId processId={processId} className="text-sm" />
          </div>
        </div>

        <div className="flex gap-3 mb-4">
          <input
            type="text"
            value={searchInputValue}
            onChange={(e) => setSearchInputValue(e.target.value)}
            onBlur={handleSearchBlur}
            onKeyDown={handleSearchKeyDown}
            placeholder="Search target or message..."
            className="w-64 px-3 py-2 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link placeholder:text-theme-text-muted"
          />

          <select
            value={logLevel}
            onChange={(e) => updateLogLevel(e.target.value)}
            className="px-3 py-2 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link"
          >
            <option value="all">Max Level: TRACE (all)</option>
            <option value="debug">Max Level: DEBUG</option>
            <option value="info">Max Level: INFO</option>
            <option value="warn">Max Level: WARN</option>
            <option value="error">Max Level: ERROR</option>
            <option value="fatal">Max Level: FATAL</option>
          </select>

          <div className="flex items-center gap-2">
            <span className="text-theme-text-muted text-sm">Limit:</span>
            <EditableCombobox
              value={limitInputValue}
              options={PRESET_LIMITS}
              onChange={(value) => setLimitInputValue(value)}
              onSelect={updateLogLimit}
              onBlur={handleLimitInputBlur}
              onKeyDown={handleLimitInputKeyDown}
            />
          </div>

          <span className="ml-auto text-xs text-theme-text-muted self-center">
            {streamQuery.isStreaming && rows.length === 0
              ? 'Loading...'
              : `Showing ${rows.length} entries`}
          </span>
        </div>

        {queryError && (
          <ErrorBanner
            title="Query execution failed"
            message={queryError}
            onRetry={streamQuery.error?.retryable ? handleRefresh : undefined}
          />
        )}

        <div className="flex-1 overflow-auto bg-app-bg border border-theme-border rounded-lg font-mono text-xs">
          {streamQuery.isStreaming && !hasLoaded ? (
            <div className="flex items-center justify-center h-full">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
                <span className="text-theme-text-secondary">Loading logs...</span>
              </div>
            </div>
          ) : rows.length === 0 ? (
            <div className="flex items-center justify-center h-full">
              <span className="text-theme-text-muted">No log entries found</span>
            </div>
          ) : (
            <div>
              {rows.map((row, index) => (
                <div
                  key={index}
                  className="flex px-3 py-1 border-b border-app-panel hover:bg-app-panel/50 transition-colors"
                >
                  <span className="text-theme-text-muted mr-3 w-[188px] min-w-[188px] whitespace-nowrap">
                    {formatLocalTime(row.time)}
                  </span>
                  <span className={`w-[38px] min-w-[38px] mr-3 font-semibold ${getLevelColor(row.level)}`}>
                    {String(row.level ?? '')}
                  </span>
                  <span
                    className="text-accent-highlight mr-3 w-[200px] min-w-[200px] truncate"
                    title={String(row.target ?? '')}
                  >
                    {String(row.target ?? '')}
                  </span>
                  <span className="text-theme-text-primary flex-1 break-words">{String(row.msg ?? '')}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </PageLayout>
  )
}

export default function ProcessLogPage() {
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
        <ProcessLogContent />
      </Suspense>
    </AuthGuard>
  )
}
