'use client'

import { Suspense, useState, useEffect, useRef } from 'react'
import { useSearchParams } from 'next/navigation'
import { useMutation } from '@tanstack/react-query'
import Link from 'next/link'
import { AlertCircle, Play } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { executeSqlQuery, toRowObjects, generateTrace } from '@/lib/api'
import { useTimeRange } from '@/hooks/useTimeRange'
import { ProgressUpdate, GenerateTraceRequest, SqlRow } from '@/types'

const PROCESS_SQL = `SELECT exe FROM processes WHERE process_id = '$process_id' LIMIT 1`

function ProcessTraceContent() {
  const searchParams = useSearchParams()
  const processId = searchParams.get('process_id')
  const { parsed: timeRange, apiTimeRange } = useTimeRange()

  const [processExe, setProcessExe] = useState<string | null>(null)
  const [includeThreadSpans, setIncludeThreadSpans] = useState(true)
  const [includeAsyncSpans, setIncludeAsyncSpans] = useState(true)
  const [isGenerating, setIsGenerating] = useState(false)
  const [progress, setProgress] = useState<ProgressUpdate | null>(null)

  const processMutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      const rows = toRowObjects(data)
      if (rows.length > 0) {
        setProcessExe(String(rows[0].exe ?? ''))
      }
    },
  })

  // Use ref to avoid including mutation in deps
  const processMutateRef = useRef(processMutation.mutate)
  processMutateRef.current = processMutation.mutate

  const hasLoadedRef = useRef(false)
  useEffect(() => {
    if (processId && !hasLoadedRef.current) {
      hasLoadedRef.current = true
      processMutateRef.current({
        sql: PROCESS_SQL,
        params: { process_id: processId },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })
    }
  }, [processId, apiTimeRange])

  const handleGenerateTrace = async () => {
    if (!processId) return

    setIsGenerating(true)
    setProgress(null)

    const request: GenerateTraceRequest = {
      include_async_spans: includeAsyncSpans,
      include_thread_spans: includeThreadSpans,
      time_range: {
        begin: timeRange.from.toISOString(),
        end: timeRange.to.toISOString(),
      },
    }

    try {
      await generateTrace(processId, request, (update) => {
        setProgress(update)
      })
    } catch (error) {
      console.error('Failed to generate trace:', error)
    } finally {
      setIsGenerating(false)
      setProgress(null)
    }
  }

  const formatTimeForDisplay = (date: Date) => {
    return date.toISOString()
  }

  if (!processId) {
    return (
      <PageLayout>
        <div className="p-6">
          <div className="flex flex-col items-center justify-center h-64 bg-app-panel border border-theme-border rounded-lg">
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
    <PageLayout>
      <div className="p-6 max-w-3xl">
        {/* Page Header */}
        <div className="mb-8">
          <h1 className="text-2xl font-semibold text-gray-200">Generate Trace</h1>
        </div>

        {/* Form */}
        <div className="bg-app-panel border border-theme-border rounded-lg p-6 mb-6">
          <h2 className="text-base font-semibold text-gray-200 mb-5 pb-3 border-b border-theme-border">
            Trace Configuration
          </h2>

          {/* Process */}
          <div className="mb-5">
            <label className="block text-sm font-medium text-gray-200 mb-2">Process</label>
            <div className="bg-app-bg border border-theme-border rounded-md p-3">
              <span className="text-gray-200 font-medium">{processExe || 'Loading...'}</span>
              <span className="text-gray-500 font-mono text-sm ml-2">
                <CopyableProcessId processId={processId} className="text-sm" />
              </span>
            </div>
          </div>

          {/* Span Types */}
          <div className="mb-5">
            <label className="block text-sm font-medium text-gray-200 mb-3">Span Types</label>
            <div className="space-y-3">
              <div>
                <label className="flex items-center gap-2.5 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={includeThreadSpans}
                    onChange={(e) => setIncludeThreadSpans(e.target.checked)}
                    className="w-4 h-4 rounded border-theme-border bg-app-bg text-blue-500 focus:ring-blue-500 focus:ring-offset-0"
                  />
                  <span className="text-sm text-gray-200">Thread Events</span>
                </label>
                <p className="text-xs text-gray-500 ml-6 mt-0.5">
                  Include synchronous span events from threads
                </p>
              </div>
              <div>
                <label className="flex items-center gap-2.5 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={includeAsyncSpans}
                    onChange={(e) => setIncludeAsyncSpans(e.target.checked)}
                    className="w-4 h-4 rounded border-theme-border bg-app-bg text-blue-500 focus:ring-blue-500 focus:ring-offset-0"
                  />
                  <span className="text-sm text-gray-200">Async Span Events</span>
                </label>
                <p className="text-xs text-gray-500 ml-6 mt-0.5">
                  Include asynchronous span events from futures
                </p>
              </div>
            </div>
          </div>

          {/* Time Range */}
          <div className="mb-5">
            <label className="block text-sm font-medium text-gray-200 mb-2">Time Range</label>
            <p className="text-sm text-gray-400">
              Uses the global time range from the header:
              <br />
              <strong className="text-gray-200">{formatTimeForDisplay(timeRange.from)}</strong>
              {' to '}
              <strong className="text-gray-200">{formatTimeForDisplay(timeRange.to)}</strong>
            </p>
          </div>

          {/* Actions */}
          <div className="flex gap-3 mt-6">
            <button
              onClick={handleGenerateTrace}
              disabled={isGenerating || (!includeThreadSpans && !includeAsyncSpans)}
              className="flex items-center gap-2 px-5 py-2.5 bg-blue-500 text-white rounded-md hover:bg-blue-600 disabled:bg-gray-600 disabled:cursor-not-allowed transition-colors text-sm font-medium"
            >
              <Play className="w-4 h-4" />
              Generate Trace
            </button>
            <Link
              href={`/process?id=${processId}`}
              className="px-5 py-2.5 bg-theme-border text-gray-200 rounded-md hover:bg-theme-border-hover transition-colors text-sm font-medium"
            >
              Cancel
            </Link>
          </div>
        </div>

        {/* Progress */}
        {isGenerating && (
          <div className="bg-app-panel border border-theme-border rounded-lg p-6">
            <div className="flex items-center gap-4">
              <div className="w-6 h-6 border-3 border-theme-border border-t-blue-500 rounded-full animate-spin" />
              <span className="text-base font-semibold text-gray-200">Generating Trace...</span>
            </div>
            {progress && (
              <p className="text-sm text-gray-400 mt-3">
                {progress.message} ({progress.percentage}%)
              </p>
            )}
          </div>
        )}
      </div>
    </PageLayout>
  )
}

export default function ProcessTracePage() {
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
        <ProcessTraceContent />
      </Suspense>
    </AuthGuard>
  )
}
