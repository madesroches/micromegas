'use client'

import { Suspense, useState, useMemo, useCallback } from 'react'
import { useQuery, useMutation } from '@tanstack/react-query'
import Link from 'next/link'
import { AlertCircle, ChevronUp, ChevronDown } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { QueryEditor } from '@/components/QueryEditor'
import { ErrorBanner } from '@/components/ErrorBanner'
import { fetchProcesses, executeSqlQuery } from '@/lib/api'
import { useTimeRange } from '@/hooks/useTimeRange'
import { ProcessInfo, SqlQueryResponse } from '@/types'

type SortField = 'exe' | 'start_time' | 'last_update_time' | 'username' | 'computer'
type SortDirection = 'asc' | 'desc'

const DEFAULT_SQL = `SELECT process_id, start_time, last_update_time, exe, computer, username
FROM processes
ORDER BY $order_by
LIMIT 100`

const VARIABLES = [
  { name: 'search', description: 'Search filter value' },
  { name: 'order_by', description: 'Sort column and direction' },
]

// Convert SQL query results to ProcessInfo format
function sqlResultToProcesses(result: SqlQueryResponse): ProcessInfo[] {
  const colIndex = (name: string) => result.columns.indexOf(name)
  const processIdIdx = colIndex('process_id')
  const exeIdx = colIndex('exe')
  const startTimeIdx = colIndex('start_time')
  const lastUpdateIdx = colIndex('last_update_time')
  const computerIdx = colIndex('computer')
  const usernameIdx = colIndex('username')

  return result.rows.map((row) => ({
    process_id: String(row[processIdIdx] ?? ''),
    exe: String(row[exeIdx] ?? ''),
    start_time: String(row[startTimeIdx] ?? ''),
    last_update_time: String(row[lastUpdateIdx] ?? ''),
    computer: String(row[computerIdx] ?? ''),
    username: String(row[usernameIdx] ?? ''),
    cpu_brand: '',
    distro: '',
    properties: {},
  }))
}

function ProcessesPageContent() {
  const [searchTerm, setSearchTerm] = useState('')
  const [sortField, setSortField] = useState<SortField>('start_time')
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc')
  const [queryError, setQueryError] = useState<string | null>(null)
  const [customSqlResults, setCustomSqlResults] = useState<ProcessInfo[] | null>(null)
  const [isUsingCustomQuery, setIsUsingCustomQuery] = useState(false)
  const { parsed: timeRange, apiTimeRange } = useTimeRange()

  const {
    data: processes = [],
    isLoading,
    error,
    refetch,
  } = useQuery({
    queryKey: ['processes'],
    queryFn: fetchProcesses,
  })

  const sqlMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      setQueryError(null)
      setCustomSqlResults(sqlResultToProcesses(data))
      setIsUsingCustomQuery(true)
    },
    onError: (err: Error) => {
      setQueryError(err.message)
      setCustomSqlResults(null)
    },
  })

  // Use custom SQL results if available, otherwise use default query results
  const dataSource = isUsingCustomQuery && customSqlResults ? customSqlResults : processes

  const filteredAndSortedProcesses = useMemo(() => {
    // If using custom query, don't apply client-side filtering/sorting (query handles it)
    if (isUsingCustomQuery && customSqlResults) {
      // Still apply search filter on custom results for convenience
      if (searchTerm) {
        return customSqlResults.filter(
          (process) =>
            process.exe.toLowerCase().includes(searchTerm.toLowerCase()) ||
            process.computer.toLowerCase().includes(searchTerm.toLowerCase()) ||
            process.username.toLowerCase().includes(searchTerm.toLowerCase()) ||
            process.process_id.toLowerCase().includes(searchTerm.toLowerCase())
        )
      }
      return customSqlResults
    }

    const filtered = processes.filter(
      (process) =>
        process.exe.toLowerCase().includes(searchTerm.toLowerCase()) ||
        process.computer.toLowerCase().includes(searchTerm.toLowerCase()) ||
        process.username.toLowerCase().includes(searchTerm.toLowerCase()) ||
        process.process_id.toLowerCase().includes(searchTerm.toLowerCase())
    )

    return filtered.sort((a, b) => {
      const aVal = a[sortField]
      const bVal = b[sortField]

      if (sortField === 'start_time' || sortField === 'last_update_time') {
        const aDate = new Date(aVal as string).getTime()
        const bDate = new Date(bVal as string).getTime()
        return sortDirection === 'asc' ? aDate - bDate : bDate - aDate
      }

      const result = String(aVal).localeCompare(String(bVal))
      return sortDirection === 'asc' ? result : -result
    })
  }, [processes, customSqlResults, isUsingCustomQuery, searchTerm, sortField, sortDirection])

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
      setQueryError(null)
      // Substitute macros in the SQL
      const params: Record<string, string> = {
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

  const handleResetQuery = useCallback(() => {
    setQueryError(null)
    setCustomSqlResults(null)
    setIsUsingCustomQuery(false)
    refetch()
  }, [refetch])

  const currentValues = useMemo(
    () => ({
      search: searchTerm || '(empty)',
      order_by: `${sortField} ${sortDirection.toUpperCase()}`,
    }),
    [searchTerm, sortField, sortDirection]
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

  const formatTimestamp = (timestamp: string) => {
    const date = new Date(timestamp)
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
      isLoading={isLoading || sqlMutation.isPending}
      error={queryError}
    />
  )

  const handleRefresh = useCallback(() => {
    if (isUsingCustomQuery) {
      // Re-run the custom query would require storing the last SQL
      // For now, just reset to default
      setCustomSqlResults(null)
      setIsUsingCustomQuery(false)
    }
    refetch()
  }, [isUsingCustomQuery, refetch])

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

          {/* Custom query indicator */}
          {isUsingCustomQuery && (
            <div className="mb-3 flex items-center gap-2 text-sm text-blue-400">
              <span className="px-2 py-0.5 bg-blue-500/20 rounded text-xs">Custom Query</span>
              <span className="text-gray-500">
                Showing {filteredAndSortedProcesses.length} results from custom SQL
              </span>
            </div>
          )}

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
          {isLoading || sqlMutation.isPending ? (
            <div className="flex-1 flex items-center justify-center bg-[#1a1f26] border border-[#2f3540] rounded-lg">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-blue-500 border-t-transparent" />
                <span className="text-gray-400">
                  {sqlMutation.isPending ? 'Executing query...' : 'Loading processes...'}
                </span>
              </div>
            </div>
          ) : error ? (
            <div className="flex-1 flex flex-col">
              <ErrorBanner
                title="Failed to load processes"
                message="Unable to connect to the analytics server. Please try again."
                details={error instanceof Error ? error.message : String(error)}
                onRetry={() => refetch()}
              />
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
                  {filteredAndSortedProcesses.map((process) => (
                    <tr
                      key={process.process_id}
                      className="border-b border-[#2f3540] hover:bg-[#22272e] transition-colors"
                    >
                      <td className="px-4 py-3">
                        <Link
                          href={`/process?id=${process.process_id}`}
                          className="text-blue-400 hover:underline"
                        >
                          {process.exe}
                        </Link>
                      </td>
                      <td className="hidden sm:table-cell px-4 py-3">
                        <CopyableProcessId
                          processId={process.process_id}
                          truncate={true}
                          className="text-sm font-mono text-gray-400"
                        />
                      </td>
                      <td className="px-4 py-3 font-mono text-sm text-gray-300">
                        {formatTimestamp(process.start_time)}
                      </td>
                      <td className="hidden lg:table-cell px-4 py-3 font-mono text-sm text-gray-300">
                        {formatTimestamp(process.last_update_time)}
                      </td>
                      <td className="hidden md:table-cell px-4 py-3 text-gray-300">{process.username}</td>
                      <td className="hidden md:table-cell px-4 py-3 text-gray-300">{process.computer}</td>
                    </tr>
                  ))}
                  {filteredAndSortedProcesses.length === 0 && (
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
