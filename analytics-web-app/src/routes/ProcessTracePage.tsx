import { Suspense, useState, useEffect, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { useMutation } from '@tanstack/react-query'
import { AppLink } from '@/components/AppLink'
import { AlertCircle, Play } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { executeSqlQuery, toRowObjects, generateTrace } from '@/lib/api'
import { useTimeRange } from '@/hooks/useTimeRange'
import { ProgressUpdate, GenerateTraceRequest } from '@/types'

const PROCESS_SQL = `SELECT exe FROM processes WHERE process_id = '$process_id' LIMIT 1`

function ProcessTraceContent() {
  const [searchParams] = useSearchParams()
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
    <PageLayout>
      <div className="p-6 max-w-3xl">
        <div className="mb-8">
          <h1 className="text-2xl font-semibold text-theme-text-primary">Generate Trace</h1>
        </div>

        <div className="bg-app-panel border border-theme-border rounded-lg p-6 mb-6">
          <h2 className="text-base font-semibold text-theme-text-primary mb-5 pb-3 border-b border-theme-border">
            Trace Configuration
          </h2>

          <div className="mb-5">
            <label className="block text-sm font-medium text-theme-text-primary mb-2">Process</label>
            <div className="bg-app-bg border border-theme-border rounded-md p-3">
              <span className="text-theme-text-primary font-medium">{processExe || 'Loading...'}</span>
              <span className="text-theme-text-muted font-mono text-sm ml-2">
                <CopyableProcessId processId={processId} className="text-sm" />
              </span>
            </div>
          </div>

          <div className="mb-5">
            <label className="block text-sm font-medium text-theme-text-primary mb-3">Span Types</label>
            <div className="space-y-3">
              <div>
                <label className="flex items-center gap-2.5 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={includeThreadSpans}
                    onChange={(e) => setIncludeThreadSpans(e.target.checked)}
                    className="w-4 h-4 rounded border-theme-border bg-app-bg text-accent-link focus:ring-accent-link focus:ring-offset-0"
                  />
                  <span className="text-sm text-theme-text-primary">Thread Events</span>
                </label>
                <p className="text-xs text-theme-text-muted ml-6 mt-0.5">
                  Include synchronous span events from threads
                </p>
              </div>
              <div>
                <label className="flex items-center gap-2.5 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={includeAsyncSpans}
                    onChange={(e) => setIncludeAsyncSpans(e.target.checked)}
                    className="w-4 h-4 rounded border-theme-border bg-app-bg text-accent-link focus:ring-accent-link focus:ring-offset-0"
                  />
                  <span className="text-sm text-theme-text-primary">Async Span Events</span>
                </label>
                <p className="text-xs text-theme-text-muted ml-6 mt-0.5">
                  Include asynchronous span events from futures
                </p>
              </div>
            </div>
          </div>

          <div className="mb-5">
            <label className="block text-sm font-medium text-theme-text-primary mb-2">Time Range</label>
            <p className="text-sm text-theme-text-secondary">
              Uses the global time range from the header:
              <br />
              <strong className="text-theme-text-primary">{formatTimeForDisplay(timeRange.from)}</strong>
              {' to '}
              <strong className="text-theme-text-primary">{formatTimeForDisplay(timeRange.to)}</strong>
            </p>
          </div>

          <div className="flex gap-3 mt-6">
            <button
              onClick={handleGenerateTrace}
              disabled={isGenerating || (!includeThreadSpans && !includeAsyncSpans)}
              className="flex items-center gap-2 px-5 py-2.5 bg-accent-link text-white rounded-md hover:bg-accent-link-hover disabled:bg-theme-border disabled:cursor-not-allowed transition-colors text-sm font-medium"
            >
              <Play className="w-4 h-4" />
              Generate Trace
            </button>
          </div>
        </div>

        {isGenerating && (
          <div className="bg-app-panel border border-theme-border rounded-lg p-6">
            <div className="flex items-center gap-4">
              <div className="w-6 h-6 border-3 border-theme-border border-t-accent-link rounded-full animate-spin" />
              <span className="text-base font-semibold text-theme-text-primary">Generating Trace...</span>
            </div>
            {progress && (
              <p className="text-sm text-theme-text-secondary mt-3">
                {progress.message}
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
                <div className="animate-spin rounded-full h-8 w-8 border-2 border-accent-link border-t-transparent" />
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
