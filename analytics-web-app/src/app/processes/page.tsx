'use client'

import { Suspense, useState, useMemo, useCallback } from 'react'
import { useQuery } from '@tanstack/react-query'
import Link from 'next/link'
import { AlertCircle, ChevronUp, ChevronDown } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { QueryEditor } from '@/components/QueryEditor'
import { fetchProcesses } from '@/lib/api'
import { useTimeRange } from '@/hooks/useTimeRange'

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

function ProcessesPageContent() {
  const [searchTerm, setSearchTerm] = useState('')
  const [sortField, setSortField] = useState<SortField>('start_time')
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc')
  const [queryError, setQueryError] = useState<string | null>(null)
  const { parsed: timeRange } = useTimeRange()

  const {
    data: processes = [],
    isLoading,
    error,
    refetch,
  } = useQuery({
    queryKey: ['processes'],
    queryFn: fetchProcesses,
  })

  const filteredAndSortedProcesses = useMemo(() => {
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
  }, [processes, searchTerm, sortField, sortDirection])

  const handleSort = (field: SortField) => {
    if (sortField === field) {
      setSortDirection(sortDirection === 'asc' ? 'desc' : 'asc')
    } else {
      setSortField(field)
      setSortDirection('desc')
    }
  }

  const handleRunQuery = useCallback((sql: string) => {
    // For now, just refetch using the default API
    // In future, this would execute the custom SQL
    setQueryError(null)
    refetch()
  }, [refetch])

  const handleResetQuery = useCallback(() => {
    setQueryError(null)
  }, [])

  const currentValues = useMemo(
    () => ({
      search: searchTerm || '(empty)',
      order_by: `${sortField} ${sortDirection.toUpperCase()}`,
    }),
    [searchTerm, sortField, sortDirection]
  )

  const SortHeader = ({ field, children }: { field: SortField; children: React.ReactNode }) => (
    <th
      onClick={() => handleSort(field)}
      className={`px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider cursor-pointer select-none transition-colors ${
        sortField === field
          ? 'text-gray-200 bg-[#2a3038]'
          : 'text-gray-500 hover:text-gray-300 hover:bg-[#2a3038]'
      }`}
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
      isLoading={isLoading}
      error={queryError}
    />
  )

  return (
    <AuthGuard>
      <PageLayout onRefresh={() => refetch()} rightPanel={sqlPanel}>
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

          {/* Table */}
          {isLoading ? (
            <div className="flex-1 flex items-center justify-center bg-[#1a1f26] border border-[#2f3540] rounded-lg">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-6 w-6 border-2 border-blue-500 border-t-transparent" />
                <span className="text-gray-400">Loading processes...</span>
              </div>
            </div>
          ) : error ? (
            <div className="flex-1 flex items-center justify-center bg-[#1a1f26] border border-[#2f3540] rounded-lg">
              <div className="flex flex-col items-center gap-3">
                <AlertCircle className="w-10 h-10 text-red-400" />
                <p className="text-gray-400">Failed to load processes</p>
              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-auto bg-[#1a1f26] border border-[#2f3540] rounded-lg">
              <table className="w-full">
                <thead className="sticky top-0">
                  <tr className="bg-[#22272e] border-b border-[#2f3540]">
                    <SortHeader field="exe">Process</SortHeader>
                    <th className="px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider text-gray-500">
                      Process ID
                    </th>
                    <SortHeader field="start_time">Start Time</SortHeader>
                    <SortHeader field="last_update_time">Last Update</SortHeader>
                    <SortHeader field="username">Username</SortHeader>
                    <SortHeader field="computer">Computer</SortHeader>
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
                      <td className="px-4 py-3">
                        <CopyableProcessId
                          processId={process.process_id}
                          truncate={true}
                          className="text-sm font-mono text-gray-400"
                        />
                      </td>
                      <td className="px-4 py-3 font-mono text-sm text-gray-300">
                        {formatTimestamp(process.start_time)}
                      </td>
                      <td className="px-4 py-3 font-mono text-sm text-gray-300">
                        {formatTimestamp(process.last_update_time)}
                      </td>
                      <td className="px-4 py-3 text-gray-300">{process.username}</td>
                      <td className="px-4 py-3 text-gray-300">{process.computer}</td>
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
