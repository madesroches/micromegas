'use client'

import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { ProgressUpdate, GenerateTraceRequest } from '@/types'
import { fetchProcesses, generateTrace } from '@/lib/api'
import { ProcessTable } from '@/components/ProcessTable'
import { TraceGenerationProgress } from '@/components/TraceGenerationProgress'
import { useApiErrorHandler } from '@/components/ErrorBoundary'
import { AuthGuard } from '@/components/AuthGuard'
import { UserMenu } from '@/components/UserMenu'
import { AlertCircle } from 'lucide-react'

export default function HomePage() {
  const [isGenerating, setIsGenerating] = useState(false)
  const [progress, setProgress] = useState<ProgressUpdate | null>(null)
  const [currentProcessId, setCurrentProcessId] = useState<string | null>(null)
  const handleApiError = useApiErrorHandler()

  // Fetch processes
  const {
    data: processes = [],
    isLoading: processesLoading,
    error: processesError,
    refetch: refetchProcesses
  } = useQuery({
    queryKey: ['processes'],
    queryFn: fetchProcesses,
  })

  const handleGenerateTrace = async (processId: string) => {
    setIsGenerating(true)
    setCurrentProcessId(processId)
    setProgress(null)

    const request: GenerateTraceRequest = {
      include_async_spans: true,
      include_thread_spans: true,
    }

    try {
      await generateTrace(processId, request, (update) => {
        setProgress(update)
      })
    } catch (error) {
      handleApiError(error)
    } finally {
      setIsGenerating(false)
      setCurrentProcessId(null)
      setProgress(null)
    }
  }

  const handleRefresh = () => {
    refetchProcesses()
  }


  return (
    <AuthGuard>
      <div className="min-h-screen" style={{ backgroundColor: '#f8fafc' }}>
        {/* Header */}
        <div className="bg-white border-b border-gray-200 shadow-sm">
          <div className="px-8 py-4 flex justify-between items-start">
            <div>
              <h1 className="text-2xl font-semibold text-gray-800">Micromegas Analytics</h1>
              <p className="text-sm text-gray-600 mt-1">Explore processes, analyze logs, and export trace data</p>
            </div>
            <UserMenu />
          </div>
        </div>

        <div className="max-w-7xl mx-auto px-8 py-8">
        {/* Tab Navigation */}
        <div className="flex border-b border-gray-200 mb-8">
          <button className="px-6 py-3 border-b-2 border-blue-500 text-blue-600 font-medium text-sm">
            Process Explorer
          </button>
        </div>

        {/* Generation Progress */}
        {(isGenerating || progress) && (
          <div className="mb-8">
            <TraceGenerationProgress
              isGenerating={isGenerating}
              progress={progress}
              processId={currentProcessId || undefined}
            />
          </div>
        )}

        {/* Main Content */}
        {processesLoading ? (
          <div className="bg-white rounded-lg border border-gray-200 shadow-sm">
            <div className="flex items-center justify-center py-12">
              <div className="animate-spin rounded-full h-8 w-8 border-2 border-blue-500 border-t-transparent mr-4" />
              <span className="text-gray-700">Loading processes...</span>
            </div>
          </div>
        ) : processesError ? (
          <div className="bg-white rounded-lg border border-gray-200 shadow-sm">
            <div className="flex flex-col items-center justify-center py-12 px-6">
              <AlertCircle className="w-12 h-12 text-red-400 mb-4" />
              <h3 className="text-lg font-semibold mb-2 text-gray-800">Failed to Load Processes</h3>
              <p className="text-gray-600 text-center">
                Unable to fetch process data. Please check your connection and try again.
              </p>
            </div>
          </div>
        ) : (
          <ProcessTable
            processes={processes}
            onGenerateTrace={handleGenerateTrace}
            isGenerating={isGenerating}
            onRefresh={handleRefresh}
          />
          )}
        </div>
      </div>
    </AuthGuard>
  )
}