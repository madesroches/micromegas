import { useCallback, useEffect, useRef, useState } from 'react'
import { Save, ChevronDown } from 'lucide-react'
import { registerRenderer, ScreenRendererProps } from './index'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { Button } from '@/components/ui/button'
import { useStreamQuery } from '@/hooks/useStreamQuery'
import { useDebounce } from '@/hooks/useDebounce'
import { timestampToDate } from '@/lib/arrow-utils'

// Variables available for log queries
const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
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

const PRESET_LIMITS = [50, 100, 200, 500, 1000]
const MIN_LIMIT = 1
const MAX_LIMIT = 10000

interface LogConfig {
  sql: string
}

interface LogRow {
  time: unknown
  level: string
  target: string
  msg: string
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

function formatLocalTime(utcTime: unknown): string {
  if (!utcTime) return ''.padEnd(29)

  const date = timestampToDate(utcTime)
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

function getLevelColor(level: unknown): string {
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

interface EditableComboboxProps {
  value: string
  options: number[]
  onChange: (value: string) => void
  onSelect: (value: number) => void
  onBlur: () => void
  onKeyDown: (e: React.KeyboardEvent<HTMLInputElement>) => void
}

function EditableCombobox({ value, options, onChange, onSelect, onBlur, onKeyDown }: EditableComboboxProps) {
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
    <div ref={containerRef} className="relative">
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

export function LogRenderer({
  config,
  onConfigChange,
  savedConfig,
  onUnsavedChange,
  timeRange,
  timeRangeLabel,
  currentValues,
  onSave,
  isSaving,
  hasUnsavedChanges,
  onSaveAs,
  saveError,
  refreshTrigger,
}: ScreenRendererProps) {
  const logConfig = config as LogConfig

  // Filter state
  const [logLevel, setLogLevel] = useState<string>('all')
  const [logLimit, setLogLimit] = useState<number>(100)
  const [limitInputValue, setLimitInputValue] = useState<string>('100')
  const [search, setSearch] = useState<string>('')
  const [searchInputValue, setSearchInputValue] = useState<string>('')
  const debouncedSearchInput = useDebounce(searchInputValue, 300)
  const [rows, setRows] = useState<LogRow[]>([])
  const [hasLoaded, setHasLoaded] = useState(false)

  // Query execution
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
      } else {
        // Query returned no results
        setRows([])
      }
      setHasLoaded(true)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [streamQuery.isComplete, streamQuery.error])

  // Refs for query execution
  const currentSqlRef = useRef<string>(logConfig.sql)
  const executeRef = useRef(streamQuery.execute)
  executeRef.current = streamQuery.execute
  // Track filters used in the last executed query
  const lastQueryFiltersRef = useRef<{ logLevel: string; logLimit: number; search: string }>({ logLevel: 'all', logLimit: 100, search: '' })

  // Execute query with filters
  const loadData = useCallback(
    (sql: string) => {
      currentSqlRef.current = sql
      onConfigChange({ ...logConfig, sql })

      // Check if SQL changed from saved version
      if (savedConfig && sql !== (savedConfig as LogConfig).sql) {
        onUnsavedChange()
      }

      // Track the filters used in this query
      lastQueryFiltersRef.current = { logLevel, logLimit, search }

      // Replace search filter placeholder in SQL
      const sqlWithSearch = sql.replace('$search_filter', expandSearchFilter(search))

      executeRef.current({
        sql: sqlWithSearch,
        params: {
          begin: timeRange.begin,
          end: timeRange.end,
          max_level: String(LOG_LEVELS[logLevel] || 6),
          limit: String(logLimit),
        },
        begin: timeRange.begin,
        end: timeRange.end,
      })
    },
    [logConfig, savedConfig, onConfigChange, onUnsavedChange, timeRange, logLevel, logLimit, search]
  )

  // Initial query execution
  const hasExecutedRef = useRef(false)
  useEffect(() => {
    if (!hasExecutedRef.current) {
      hasExecutedRef.current = true
      loadData(logConfig.sql)
    }
  }, []) // eslint-disable-line react-hooks/exhaustive-deps

  // Re-execute on time range change
  const prevTimeRangeRef = useRef<{ begin: string; end: string } | null>(null)
  useEffect(() => {
    if (prevTimeRangeRef.current === null) {
      prevTimeRangeRef.current = { begin: timeRange.begin, end: timeRange.end }
      return
    }
    if (
      prevTimeRangeRef.current.begin !== timeRange.begin ||
      prevTimeRangeRef.current.end !== timeRange.end
    ) {
      prevTimeRangeRef.current = { begin: timeRange.begin, end: timeRange.end }
      loadData(currentSqlRef.current)
    }
  }, [timeRange, loadData])

  // Re-execute on refresh trigger
  const prevRefreshTriggerRef = useRef(refreshTrigger)
  useEffect(() => {
    if (prevRefreshTriggerRef.current !== refreshTrigger) {
      prevRefreshTriggerRef.current = refreshTrigger
      loadData(currentSqlRef.current)
    }
  }, [refreshTrigger, loadData])

  // Re-execute on filter changes (including after initial load completes)
  useEffect(() => {
    if (!hasLoaded) return
    const lastFilters = lastQueryFiltersRef.current
    if (lastFilters.logLevel !== logLevel || lastFilters.logLimit !== logLimit || lastFilters.search !== search) {
      loadData(currentSqlRef.current)
    }
  }, [logLevel, logLimit, search, hasLoaded, loadData])

  // Update search when debounced input changes
  const isInitialSearchRef = useRef(true)
  useEffect(() => {
    if (isInitialSearchRef.current) {
      isInitialSearchRef.current = false
      return
    }
    setSearch(debouncedSearchInput)
  }, [debouncedSearchInput])

  const handleRunQuery = useCallback(
    (sql: string) => {
      loadData(sql)
    },
    [loadData]
  )

  const handleResetQuery = useCallback(() => {
    if (savedConfig) {
      loadData((savedConfig as LogConfig).sql)
    } else {
      loadData(logConfig.sql)
    }
  }, [savedConfig, logConfig.sql, loadData])

  const handleSqlChange = useCallback(
    (sql: string) => {
      if (savedConfig && sql !== (savedConfig as LogConfig).sql) {
        onUnsavedChange()
      }
    },
    [savedConfig, onUnsavedChange]
  )

  // Limit input handlers
  const handleLimitInputBlur = useCallback(() => {
    const parsed = parseInt(limitInputValue, 10)
    if (isNaN(parsed) || parsed < MIN_LIMIT) {
      setLimitInputValue(String(logLimit))
    } else {
      const clamped = Math.min(parsed, MAX_LIMIT)
      setLogLimit(clamped)
      setLimitInputValue(String(clamped))
    }
  }, [limitInputValue, logLimit])

  const handleLimitInputKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter') {
        e.currentTarget.blur()
      }
    },
    []
  )

  const handleSearchBlur = useCallback(() => {
    if (searchInputValue !== search) {
      setSearch(searchInputValue)
    }
  }, [searchInputValue, search])

  const handleSearchKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter') {
        e.currentTarget.blur()
      }
    },
    []
  )

  // Extended current values for the query editor
  const extendedCurrentValues = {
    ...currentValues,
    max_level: String(LOG_LEVELS[logLevel] || 6),
    limit: String(logLimit),
    search_filter: expandSearchFilter(search) || '(empty)',
  }

  // Query editor panel
  const sqlPanel = (
    <QueryEditor
      defaultSql={savedConfig ? (savedConfig as LogConfig).sql : logConfig.sql}
      variables={VARIABLES}
      currentValues={extendedCurrentValues}
      timeRangeLabel={timeRangeLabel}
      onRun={handleRunQuery}
      onReset={handleResetQuery}
      onChange={handleSqlChange}
      isLoading={streamQuery.isStreaming}
      error={queryError}
      docLink={{
        url: 'https://madesroches.github.io/micromegas/docs/query-guide/schema-reference/#log_entries',
        label: 'log_entries schema reference',
      }}
      footer={
        <>
          <div className="border-t border-theme-border p-3 flex gap-2">
            {onSave && (
              <Button
                variant="default"
                size="sm"
                onClick={onSave}
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
              onClick={onSaveAs}
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
  )

  // Render log content
  const renderContent = () => {
    // Loading state
    if (streamQuery.isStreaming && !hasLoaded) {
      return (
        <div className="flex-1 flex items-center justify-center bg-app-bg border border-theme-border rounded-lg">
          <div className="flex items-center gap-3">
            <div className="animate-spin rounded-full h-5 w-5 border-2 border-accent-link border-t-transparent" />
            <span className="text-theme-text-secondary">Loading logs...</span>
          </div>
        </div>
      )
    }

    // Empty state
    if (rows.length === 0) {
      return (
        <div className="flex-1 flex items-center justify-center bg-app-bg border border-theme-border rounded-lg">
          <span className="text-theme-text-muted">No log entries found</span>
        </div>
      )
    }

    // Log entries display
    return (
      <div className="flex-1 overflow-auto bg-app-bg border border-theme-border rounded-lg font-mono text-xs">
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
    )
  }

  // Handle retry
  const handleRetry = useCallback(() => {
    loadData(currentSqlRef.current)
  }, [loadData])

  return (
    <div className="flex h-full">
      <div className="flex-1 flex flex-col p-6 min-w-0">
        {/* Filter controls */}
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
            onChange={(e) => setLogLevel(e.target.value)}
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
              onSelect={(value) => {
                setLogLimit(value)
                setLimitInputValue(String(value))
              }}
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
            onRetry={streamQuery.error?.retryable ? handleRetry : undefined}
          />
        )}
        {renderContent()}
      </div>
      {sqlPanel}
    </div>
  )
}

// Register this renderer
registerRenderer('log', LogRenderer)
