'use client'

import { Suspense } from 'react'
import { useSearchParams } from 'next/navigation'
import { useQuery } from '@tanstack/react-query'
import Link from 'next/link'
import { ArrowLeft, FileText, Activity, AlertCircle } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { fetchProcesses, fetchProcessStatistics } from '@/lib/api'

function ProcessPageContent() {
  const searchParams = useSearchParams()
  const processId = searchParams.get('id')

  const { data: processes = [], isLoading: processesLoading } = useQuery({
    queryKey: ['processes'],
    queryFn: fetchProcesses,
  })

  const process = processes.find((p) => p.process_id === processId)

  const { data: statistics, refetch: refetchStatistics } = useQuery({
    queryKey: ['statistics', processId],
    queryFn: () => fetchProcessStatistics(processId!),
    enabled: !!processId && !!process,
  })

  const formatDuration = (startTime: string, endTime: string): string => {
    const start = new Date(startTime)
    const end = new Date(endTime)
    const diffMs = end.getTime() - start.getTime()

    if (diffMs < 0) return 'Invalid'

    const totalSeconds = Math.floor(diffMs / 1000)
    const seconds = totalSeconds % 60
    const minutes = Math.floor(totalSeconds / 60) % 60
    const hours = Math.floor(totalSeconds / 3600) % 24
    const days = Math.floor(totalSeconds / 86400)

    if (days > 0) {
      return `${days}d ${hours}h ${minutes}m`
    } else if (hours > 0) {
      return `${hours}h ${minutes}m ${seconds}s`
    } else if (minutes > 0) {
      return `${minutes}m ${seconds}s`
    } else {
      return `${seconds}s`
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

  if (processesLoading) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex items-center justify-center h-64 bg-[#1a1f26] border border-[#2f3540] rounded-lg">
            <div className="flex items-center gap-3">
              <div className="animate-spin rounded-full h-6 w-6 border-2 border-blue-500 border-t-transparent" />
              <span className="text-gray-400">Loading process...</span>
            </div>
          </div>
        </div>
      </PageLayout>
    )
  }

  if (!process) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-[#1a1f26] border border-[#2f3540] rounded-lg">
            <AlertCircle className="w-10 h-10 text-red-400 mb-3" />
            <p className="text-gray-400">Process not found</p>
            <Link href="/processes" className="text-blue-400 hover:underline mt-2">
              Back to Processes
            </Link>
          </div>
        </div>
      </PageLayout>
    )
  }

  return (
    <PageLayout onRefresh={() => refetchStatistics()}>
      <div className="p-6 max-w-6xl">
        {/* Back Link */}
        <Link
          href="/processes"
          className="inline-flex items-center gap-1.5 text-blue-400 hover:underline text-sm mb-4"
        >
          <ArrowLeft className="w-3 h-3" />
          All Processes
        </Link>

        {/* Page Header */}
        <div className="flex items-start justify-between mb-8">
          <div>
            <h1 className="text-2xl font-semibold text-gray-200">{process.exe}</h1>
            <div className="text-sm text-gray-500 font-mono mt-1">
              <CopyableProcessId processId={process.process_id} className="text-sm" />
            </div>
          </div>
          <div className="flex gap-3">
            <Link
              href={`/process_log?process_id=${process.process_id}`}
              className="flex items-center gap-2 px-4 py-2 bg-[#2f3540] text-gray-200 rounded-md hover:bg-[#3d4450] transition-colors text-sm"
            >
              <FileText className="w-4 h-4" />
              View Log
            </Link>
            <Link
              href={`/process_trace?process_id=${process.process_id}`}
              className="flex items-center gap-2 px-4 py-2 bg-blue-500 text-white rounded-md hover:bg-blue-600 transition-colors text-sm"
            >
              <Activity className="w-4 h-4" />
              Generate Trace
            </Link>
          </div>
        </div>

        {/* Info Cards Grid */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-5 mb-8">
          {/* Process Information */}
          <div className="bg-[#1a1f26] border border-[#2f3540] rounded-lg p-5">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-gray-500 mb-4">
              Process Information
            </h3>
            <div className="space-y-0">
              <InfoRow label="Executable" value={process.exe} />
              <InfoRow label="Process ID" value={process.process_id} mono />
              <InfoRow
                label="Properties"
                value={
                  Object.keys(process.properties).length > 0
                    ? Object.entries(process.properties)
                        .map(([k, v]) => `${k}: ${v}`)
                        .join(', ')
                    : 'None'
                }
              />
            </div>
          </div>

          {/* Environment */}
          <div className="bg-[#1a1f26] border border-[#2f3540] rounded-lg p-5">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-gray-500 mb-4">
              Environment
            </h3>
            <div className="space-y-0">
              <InfoRow label="Computer" value={process.computer} />
              <InfoRow label="Username" value={process.username} />
              <InfoRow label="Distro" value={process.distro} />
              <InfoRow label="CPU Brand" value={process.cpu_brand} />
            </div>
          </div>

          {/* Timing */}
          <div className="bg-[#1a1f26] border border-[#2f3540] rounded-lg p-5">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-gray-500 mb-4">
              Timing
            </h3>
            <div className="space-y-0">
              <InfoRow label="Start Time" value={formatTimestamp(process.start_time)} mono />
              <InfoRow label="Last Activity" value={formatTimestamp(process.last_update_time)} mono />
              <InfoRow
                label="Duration"
                value={formatDuration(process.start_time, process.last_update_time)}
              />
            </div>
          </div>

          {/* Statistics */}
          <div className="bg-[#1a1f26] border border-[#2f3540] rounded-lg p-5">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-gray-500 mb-4">
              Statistics
            </h3>
            <div className="space-y-0">
              <InfoRow
                label="Log Entries"
                value={statistics?.log_entries?.toLocaleString() || '0'}
              />
              <InfoRow label="Measures" value={statistics?.measures?.toLocaleString() || '0'} />
              <InfoRow
                label="Trace Events"
                value={statistics?.trace_events?.toLocaleString() || '0'}
              />
              <InfoRow
                label="Thread Count"
                value={statistics?.thread_count?.toLocaleString() || '0'}
              />
            </div>
          </div>
        </div>
      </div>
    </PageLayout>
  )
}

function InfoRow({
  label,
  value,
  mono = false,
  highlight = false,
}: {
  label: string
  value: string
  mono?: boolean
  highlight?: boolean
}) {
  return (
    <div className="flex justify-between py-2 border-b border-[#2f3540] last:border-b-0">
      <span className="text-gray-500 text-sm">{label}</span>
      <span
        className={`text-sm text-right max-w-[60%] break-all ${
          mono ? 'font-mono' : ''
        } ${highlight ? 'text-blue-400' : 'text-gray-200'}`}
      >
        {value}
      </span>
    </div>
  )
}

export default function ProcessPage() {
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
        <ProcessPageContent />
      </Suspense>
    </AuthGuard>
  )
}
