'use client'

import { Suspense, useState, useCallback, useMemo, useEffect, useRef } from 'react'
import { useSearchParams, useRouter, usePathname } from 'next/navigation'
import { useMutation } from '@tanstack/react-query'
import Link from 'next/link'
import { ArrowLeft, AlertCircle, ChevronDown } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { executeSqlQuery, toRowObjects } from '@/lib/api'
import { useTimeRange } from '@/hooks/useTimeRange'
import { SqlRow } from '@/types'

const DEFAULT_SQL = `SELECT time, level, target, msg
FROM log_entries
WHERE process_id = '$process_id'
  AND level <= $max_level
ORDER BY time DESC
LIMIT $limit`

const PROCESS_SQL = `SELECT exe FROM processes WHERE process_id = '$process_id' LIMIT 1`

const VARIABLES = [
  { name: 'process_id', description: 'Current process ID' },
  { name: 'max_level', description: 'Max log level filter (1-6)' },
  { name: 'limit', description: 'Row limit' },
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

function formatLocalTime(utcTime: unknown): string {
  if (!utcTime) return ''.padEnd(29)
  const str = String(utcTime)

  // Extract nanoseconds from the original string (JS Date only has ms precision)
  let nanoseconds = '000000000'
  const nanoMatch = str.match(/\.(\d+)/)
  if (nanoMatch) {
    nanoseconds = nanoMatch[1].padEnd(9, '0').slice(0, 9)
  }

  const date = new Date(str)
  if (isNaN(date.getTime())) return str.slice(0, 29).padEnd(29)

  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  const seconds = String(date.getSeconds()).padStart(2, '0')

  return `${year}-${month}-${day} ${hours}:${minutes}:${seconds}.${nanoseconds}`
}

function ProcessLogContent() {
  const searchParams = useSearchParams()
  const router = useRouter()
  const pathname = usePathname()
  const processId = searchParams.get('process_id')
  const { parsed: timeRange, apiTimeRange } = useTimeRange()

  // Read initial values from URL params with validation
  const levelParam = searchParams.get('level')
  const limitParam = searchParams.get('limit')
  const initialLevel = levelParam && VALID_LEVELS.includes(levelParam) ? levelParam : 'all'
  const initialLimit = parseLimit(limitParam)

  const [logLevel, setLogLevel] = useState<string>(initialLevel)
  const [logLimit, setLogLimit] = useState<number>(initialLimit)
  const [limitInputValue, setLimitInputValue] = useState<string>(String(initialLimit))
  const [queryError, setQueryError] = useState<string | null>(null)
  const [rows, setRows] = useState<SqlRow[]>([])
  const [processExe, setProcessExe] = useState<string | null>(null)
  const [hasLoaded, setHasLoaded] = useState(false)

  const sqlMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      setQueryError(null)
      const resultRows = toRowObjects(data)
      // Normalize level values
      setRows(resultRows.map(row => ({
        ...row,
        level: typeof row.level === 'number' ? (LEVEL_NAMES[row.level] || 'UNKNOWN') : row.level
      })))
      setHasLoaded(true)
    },
    onError: (err: Error) => {
      setQueryError(err.message)
    },
  })

  const processMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      const resultRows = toRowObjects(data)
      if (resultRows.length > 0) {
        setProcessExe(String(resultRows[0].exe ?? ''))
      }
    },
  })

  // Use refs to avoid including mutations in callback deps
  const sqlMutateRef = useRef(sqlMutation.mutate)
  sqlMutateRef.current = sqlMutation.mutate
  const processMutateRef = useRef(processMutation.mutate)
  processMutateRef.current = processMutation.mutate

  const loadData = useCallback(
    (sql: string = DEFAULT_SQL) => {
      if (!processId) return
      setQueryError(null)
      const params: Record<string, string> = {
        process_id: processId,
        max_level: String(LOG_LEVELS[logLevel] || 6),
        limit: String(logLimit),
      }
      sqlMutateRef.current({
        sql,
        params,
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    },
    [processId, logLevel, logLimit, apiTimeRange]
  )

  // Update URL when filters change
  const updateLogLevel = useCallback(
    (level: string) => {
      setLogLevel(level)
      const params = new URLSearchParams(searchParams.toString())
      if (level === 'all') {
        params.delete('level')
      } else {
        params.set('level', level)
      }
      router.push(`${pathname}?${params.toString()}`)
    },
    [searchParams, router, pathname]
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
      router.push(`${pathname}?${params.toString()}`)
    },
    [searchParams, router, pathname]
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

  // Load process info once
  const hasLoadedProcessRef = useRef(false)
  useEffect(() => {
    if (processId && !hasLoadedProcessRef.current) {
      hasLoadedProcessRef.current = true
      processMutateRef.current({
        sql: PROCESS_SQL,
        params: { process_id: processId },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    }
  }, [processId, apiTimeRange])

  // Initial load
  const hasInitialLoadRef = useRef(false)
  useEffect(() => {
    if (processId && !hasInitialLoadRef.current) {
      hasInitialLoadRef.current = true
      loadData()
    }
  }, [processId, loadData])

  // Reload when filters change (only after initial load)
  const prevFiltersRef = useRef<{ logLevel: string; logLimit: number } | null>(null)
  useEffect(() => {
    // Skip if we haven't done initial load yet
    if (!hasLoaded) return

    // Initialize ref on first run after initial load
    if (prevFiltersRef.current === null) {
      prevFiltersRef.current = { logLevel, logLimit }
      return
    }

    // Check if filters actually changed
    if (prevFiltersRef.current.logLevel !== logLevel || prevFiltersRef.current.logLimit !== logLimit) {
      prevFiltersRef.current = { logLevel, logLimit }
      loadData()
    }
  }, [logLevel, logLimit, hasLoaded, loadData])

  // Reload when time range changes (only after initial load)
  const prevTimeRangeRef = useRef<{ begin: string; end: string } | null>(null)
  useEffect(() => {
    // Skip if we haven't done initial load yet
    if (!hasLoaded) return

    // Initialize ref on first run after initial load
    if (prevTimeRangeRef.current === null) {
      prevTimeRangeRef.current = { begin: apiTimeRange.begin, end: apiTimeRange.end }
      return
    }

    // Check if time range actually changed
    if (prevTimeRangeRef.current.begin !== apiTimeRange.begin || prevTimeRangeRef.current.end !== apiTimeRange.end) {
      prevTimeRangeRef.current = { begin: apiTimeRange.begin, end: apiTimeRange.end }
      loadData()
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
    }),
    [processId, logLevel, logLimit]
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
      isLoading={sqlMutation.isPending}
      error={queryError}
    />
  ) : undefined

  const handleRefresh = useCallback(() => {
    loadData()
  }, [loadData])

  if (!processId) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-app-panel border border-theme-border rounded-lg">
            <AlertCircle className="w-10 h-10 text-accent-error mb-3" />
            <p className="text-theme-text-secondary">No process ID provided</p>
            <Link href="/processes" className="text-accent-link hover:underline mt-2">
              Back to Processes
            </Link>
          </div>
        </div>
      </PageLayout>
    )
  }

  return (
    <PageLayout onRefresh={handleRefresh} rightPanel={sqlPanel}>
      <div className="p-6 flex flex-col h-full">
        {/* Back Link */}
        <Link
          href={`/process?id=${processId}`}
          className="inline-flex items-center gap-1.5 text-accent-link hover:underline text-sm mb-4"
        >
          <ArrowLeft className="w-3 h-3" />
          {processExe || 'Process'}
        </Link>

        {/* Page Header */}
        <div className="mb-5">
          <h1 className="text-2xl font-semibold text-theme-text-primary">Process Log</h1>
          <div className="text-sm text-theme-text-muted font-mono mt-1">
            <CopyableProcessId processId={processId} className="text-sm" />
          </div>
        </div>

        {/* Filters */}
        <div className="flex gap-3 mb-4">
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
            {sqlMutation.isPending && rows.length === 0
              ? 'Loading...'
              : `Showing ${rows.length} entries`}
          </span>
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

        {/* Log Viewer */}
        <div className="flex-1 overflow-auto bg-app-bg border border-theme-border rounded-lg font-mono text-xs">
          {sqlMutation.isPending && !hasLoaded ? (
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
