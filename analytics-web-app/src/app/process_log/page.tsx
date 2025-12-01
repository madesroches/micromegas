'use client'

import { Suspense, useState } from 'react'
import { useSearchParams } from 'next/navigation'
import { useQuery } from '@tanstack/react-query'
import Link from 'next/link'
import { ArrowLeft, AlertCircle } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { fetchProcesses, fetchProcessLogEntries } from '@/lib/api'

function ProcessLogContent() {
  const searchParams = useSearchParams()
  const processId = searchParams.get('process_id')

  const [logLevel, setLogLevel] = useState<string>('all')
  const [logLimit, setLogLimit] = useState<number>(100)

  const { data: processes = [] } = useQuery({
    queryKey: ['processes'],
    queryFn: fetchProcesses,
  })

  const process = processes.find((p) => p.process_id === processId)

  const {
    data: logEntries = [],
    isLoading: logsLoading,
    refetch: refetchLogs,
  } = useQuery({
    queryKey: ['logs', processId, logLevel, logLimit],
    queryFn: () => fetchProcessLogEntries(processId!, logLevel, logLimit),
    enabled: !!processId,
    staleTime: 0,
  })

  const getLevelColor = (level: string) => {
    switch (level) {
      case 'FATAL':
        return 'text-red-600'
      case 'ERROR':
        return 'text-red-400'
      case 'WARN':
        return 'text-yellow-400'
      case 'INFO':
        return 'text-blue-400'
      case 'DEBUG':
        return 'text-gray-400'
      case 'TRACE':
        return 'text-gray-500'
      default:
        return 'text-gray-300'
    }
  }

  const formatTimestamp = (timestamp: string) => {
    return timestamp
  }

  if (!processId) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-[#1a1f26] border border-[#2f3540] rounded-lg">
            <AlertCircle className="w-10 h-10 text-red-400 mb-3" />
            <p className="text-gray-400">No process ID provided</p>
            <Link href="/processes" className="text-blue-400 hover:underline mt-2">
              Back to Processes
            </Link>
          </div>
        </div>
      </PageLayout>
    )
  }

  return (
    <PageLayout onRefresh={() => refetchLogs()}>
      <div className="p-6 flex flex-col h-full">
        {/* Back Link */}
        <Link
          href={`/process?id=${processId}`}
          className="inline-flex items-center gap-1.5 text-blue-400 hover:underline text-sm mb-4"
        >
          <ArrowLeft className="w-3 h-3" />
          {process?.exe || 'Process'}
        </Link>

        {/* Page Header */}
        <div className="mb-5">
          <h1 className="text-2xl font-semibold text-gray-200">Process Log</h1>
          <div className="text-sm text-gray-500 font-mono mt-1">
            <CopyableProcessId processId={processId} className="text-sm" />
          </div>
        </div>

        {/* Filters */}
        <div className="flex gap-3 mb-4">
          <select
            value={logLevel}
            onChange={(e) => setLogLevel(e.target.value)}
            className="px-3 py-2 bg-[#1a1f26] border border-[#2f3540] rounded-md text-gray-200 text-sm focus:outline-none focus:border-blue-500"
          >
            <option value="all">Max Level: TRACE (all)</option>
            <option value="debug">Max Level: DEBUG</option>
            <option value="info">Max Level: INFO</option>
            <option value="warn">Max Level: WARN</option>
            <option value="error">Max Level: ERROR</option>
            <option value="fatal">Max Level: FATAL</option>
          </select>

          <div className="flex items-center gap-2">
            <span className="text-gray-500 text-sm">Limit:</span>
            <select
              value={logLimit}
              onChange={(e) => setLogLimit(Number(e.target.value))}
              className="px-3 py-2 bg-[#1a1f26] border border-[#2f3540] rounded-md text-gray-200 text-sm focus:outline-none focus:border-blue-500"
            >
              <option value={50}>50</option>
              <option value={100}>100</option>
              <option value={200}>200</option>
              <option value={500}>500</option>
              <option value={1000}>1000</option>
            </select>
          </div>

          <span className="ml-auto text-xs text-gray-500 self-center">
            {logsLoading ? 'Loading...' : `Showing ${logEntries.length} entries`}
          </span>
        </div>

        {/* Log Viewer */}
        <div className="flex-1 overflow-auto bg-[#0d1117] border border-[#2f3540] rounded-lg font-mono text-xs">
          {logsLoading ? (
            <div className="flex items-center justify-center h-full">
              <div className="flex items-center gap-3">
                <div className="animate-spin rounded-full h-5 w-5 border-2 border-blue-500 border-t-transparent" />
                <span className="text-gray-400">Loading logs...</span>
              </div>
            </div>
          ) : logEntries.length === 0 ? (
            <div className="flex items-center justify-center h-full">
              <span className="text-gray-500">No log entries found</span>
            </div>
          ) : (
            <div>
              {logEntries.map((log, index) => (
                <div
                  key={index}
                  className="flex px-3 py-1 border-b border-[#1a1f26] hover:bg-[#161b22] transition-colors"
                >
                  <span className="text-gray-500 mr-4 whitespace-nowrap">
                    {formatTimestamp(log.time)}
                  </span>
                  <span className={`w-12 mr-3 font-semibold ${getLevelColor(log.level)}`}>
                    {log.level}
                  </span>
                  <span
                    className="text-purple-400 mr-3 max-w-[200px] truncate"
                    title={log.target}
                  >
                    {log.target}
                  </span>
                  <span className="text-gray-200 flex-1 break-words">{log.msg}</span>
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
                <div className="animate-spin rounded-full h-8 w-8 border-2 border-blue-500 border-t-transparent" />
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
