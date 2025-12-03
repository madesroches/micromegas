'use client'

import { Suspense, useState, useMemo, useCallback, useEffect, useRef } from 'react'
import { useMutation } from '@tanstack/react-query'
import Link from 'next/link'
import { ChevronUp, ChevronDown } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { executeSqlQuery, toRowObjects } from '@/lib/api'
import { useTimeRange } from '@/hooks/useTimeRange'
import { formatTimestamp } from '@/lib/time-range'
import { SqlRow } from '@/types'

type SortField = 'exe' | 'start_time' | 'last_update_time' | 'username' | 'computer'
type SortDirection = 'asc' | 'desc'

const DEFAULT_SQL = `SELECT process_id, start_time, last_update_time, exe, computer, username
FROM processes
WHERE exe LIKE '%$search%'
   OR computer LIKE '%$search%'
   OR username LIKE '%$search%'
ORDER BY $order_by
LIMIT 100`

const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
  { name: 'order_by', description: 'Sort column and direction' },
  { name: 'search', description: 'Search filter value' },
]

function ProcessesPageContent() {
  const [searchInput, setSearchInput] = useState('')
  const [searchTerm, setSearchTerm] = useState('')
  const [sortField, setSortField] = useState<SortField>('last_update_time')
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc')
  const [queryError, setQueryError] = useState<string | null>(null)
  const [rows, setRows] = useState<SqlRow[]>([])
  const { parsed: timeRange, apiTimeRange } = useTimeRange()

  // Debounce search input
  useEffect(() => {
    const timer = setTimeout(() => {
      setSearchTerm(searchInput)
    }, 300)
    return () => clearTimeout(timer)
  }, [searchInput])

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

  const loadData = useCallback(
    (sql: string = DEFAULT_SQL) => {
      setQueryError(null)
      const params: Record<string, string> = {
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
        order_by: `${sortField} ${sortDirection.toUpperCase()}`,
        search: searchTerm,
      }
      mutateRef.current({
        sql,
        params,
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    },
    [sortField, sortDirection, searchTerm, apiTimeRange]
  )

  // Load on mount and when time range, sort, or search changes
  const queryKey = `${apiTimeRange.begin}-${apiTimeRange.end}-${sortField}-${sortDirection}-${searchTerm}`
  const prevQueryKeyRef = useRef<string | null>(null)
  useEffect(() => {
    if (prevQueryKeyRef.current !== queryKey) {
      prevQueryKeyRef.current = queryKey
      loadData()
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
    () => ({
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
      order_by: `${sortField} ${sortDirection.toUpperCase()}`,
      search: searchTerm || '(empty)',
    }),
    [apiTimeRange, searchTerm, sortField, sortDirection]
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
    />
  )

  const handleRefresh = useCallback(() => {
    loadData()
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
                        <Link
                          href={`/process?id=${row.process_id}`}
                          className="text-accent-link hover:underline"
                        >
                          {String(row.exe ?? '')}
                        </Link>
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
                      <td colSpan={6} className="px-4 py-8 text-center text-theme-text-muted">
                        {searchTerm ? 'No processes match your search.' : 'No processes available.'}
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
