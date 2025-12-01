'use client'

import { Suspense, useState, useMemo, useCallback } from 'react'
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
import { SqlRow } from '@/types'

type SortField = 'exe' | 'start_time' | 'last_update_time' | 'username' | 'computer'
type SortDirection = 'asc' | 'desc'

const DEFAULT_SQL = `SELECT process_id, start_time, last_update_time, exe, computer, username
FROM processes
WHERE start_time <= '$end' AND last_update_time >= '$begin'
ORDER BY $order_by
LIMIT 100`

const VARIABLES = [
  { name: 'begin', description: 'Time range start (ISO timestamp)' },
  { name: 'end', description: 'Time range end (ISO timestamp)' },
  { name: 'search', description: 'Search filter value' },
  { name: 'order_by', description: 'Sort column and direction' },
]

function ProcessesPageContent() {
  const [searchTerm, setSearchTerm] = useState('')
  const [sortField, setSortField] = useState<SortField>('start_time')
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc')
  const [queryError, setQueryError] = useState<string | null>(null)
  const [rows, setRows] = useState<SqlRow[]>([])
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

  // Load data on first render
  const loadData = useCallback(
    (sql: string = DEFAULT_SQL) => {
      setQueryError(null)
      const params: Record<string, string> = {
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
        order_by: `${sortField} ${sortDirection.toUpperCase()}`,
        search: searchTerm,
      }
      sqlMutation.mutate({
        sql,
        params,
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    },
    [sqlMutation, sortField, sortDirection, searchTerm, apiTimeRange]
  )

  // Load on mount and when time range changes
  const timeRangeKey = `${apiTimeRange.begin}-${apiTimeRange.end}`
  useMemo(() => {
    if (!sqlMutation.isPending) {
      loadData()
    }
  }, [timeRangeKey]) // eslint-disable-line react-hooks/exhaustive-deps

  const filteredRows = useMemo(() => {
    if (!searchTerm) return rows

    const term = searchTerm.toLowerCase()
    return rows.filter((row) =>
      ['exe', 'computer', 'username', 'process_id'].some((field) =>
        String(row[field] ?? '').toLowerCase().includes(term)
      )
    )
  }, [rows, searchTerm])

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
      search: searchTerm || '(empty)',
      order_by: `${sortField} ${sortDirection.toUpperCase()}`,
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
          ? 'text-gray-200 bg-[#2a3038]'
          : 'text-gray-500 hover:text-gray-300 hover:bg-[#2a3038]'
      } ${className}`}
    >
      <div className="flex items-center gap-1">
        {children}
        <span className={sortField === field ? 'text-blue-500' : 'opacity-30'}>
          {sortField === field && sortDirection === 'asc' ? (
            <ChevronUp className="w-3 h-3" />
          ) : (
            <ChevronDown className="w-3 h-3" />
          )}
        </span>
      </div>
    </th>
  )

  const formatTimestamp = (value: unknown) => {
    if (!value) return ''
    const date = new Date(String(value))
    return date.toISOString().replace('T', ' ').slice(0, 23) + 'Z'
  }

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
            <h1 className="text-2xl font-semibold text-gray-200">Processes</h1>
          </div>

          {/* Search */}
          <div className="mb-4">
            <input
              type="text"
              placeholder="Search by exe, process_id, computer, username..."
              value={searchTerm}
              onChange={(e) => setSearchTerm(e.target.value)}
              className="w-full max-w-md px-4 py-2.5 bg-[#1a1f26] border border-[#2f3540] rounded-md text-gray-200 text-sm placeholder-gray-500 focus:outline-none focus:border-blue-500 transition-colors"
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
            <div className="flex-1 flex items-center justify-center bg-[#1a1f26] border border-[#2f3540] rounded-lg">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-blue-500 border-t-transparent" />
                <span className="text-gray-400">Loading processes...</span>
              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-auto bg-[#1a1f26] border border-[#2f3540] rounded-lg">
              <table className="w-full">
                <thead className="sticky top-0">
                  <tr className="bg-[#22272e] border-b border-[#2f3540]">
                    <SortHeader field="exe">Process</SortHeader>
                    <th className="hidden sm:table-cell px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider text-gray-500">
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
                  {filteredRows.map((row) => (
                    <tr
                      key={String(row.process_id)}
                      className="border-b border-[#2f3540] hover:bg-[#22272e] transition-colors"
                    >
                      <td className="px-4 py-3">
                        <Link
                          href={`/process?id=${row.process_id}`}
                          className="text-blue-400 hover:underline"
                        >
                          {String(row.exe ?? '')}
                        </Link>
                      </td>
                      <td className="hidden sm:table-cell px-4 py-3">
                        <CopyableProcessId
                          processId={String(row.process_id ?? '')}
                          truncate={true}
                          className="text-sm font-mono text-gray-400"
                        />
                      </td>
                      <td className="px-4 py-3 font-mono text-sm text-gray-300">
                        {formatTimestamp(row.start_time)}
                      </td>
                      <td className="hidden lg:table-cell px-4 py-3 font-mono text-sm text-gray-300">
                        {formatTimestamp(row.last_update_time)}
                      </td>
                      <td className="hidden md:table-cell px-4 py-3 text-gray-300">
                        {String(row.username ?? '')}
                      </td>
                      <td className="hidden md:table-cell px-4 py-3 text-gray-300">
                        {String(row.computer ?? '')}
                      </td>
                    </tr>
                  ))}
                  {filteredRows.length === 0 && (
                    <tr>
                      <td colSpan={6} className="px-4 py-8 text-center text-gray-500">
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
                <div className="animate-spin rounded-full h-8 w-8 border-2 border-blue-500 border-t-transparent" />
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
