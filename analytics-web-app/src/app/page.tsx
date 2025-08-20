'use client'

import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { ProcessInfo, ProgressUpdate, GenerateTraceRequest } from '@/types'
import { fetchProcesses, fetchHealthCheck, generateTrace } from '@/lib/api'
import { ProcessTable } from '@/components/ProcessTable'
import { TraceGenerationProgress } from '@/components/TraceGenerationProgress'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { AlertCircle, RefreshCw, Activity, Database, Zap } from 'lucide-react'

export default function HomePage() {
  const [isGenerating, setIsGenerating] = useState(false)
  const [progress, setProgress] = useState<ProgressUpdate | null>(null)
  const [currentProcessId, setCurrentProcessId] = useState<string | null>(null)

  // Fetch health status
  const { data: health, isLoading: healthLoading } = useQuery({
    queryKey: ['health'],
    queryFn: fetchHealthCheck,
    refetchInterval: 30000, // Refetch every 30 seconds
  })

  // Fetch processes
  const { 
    data: processes = [], 
    isLoading: processesLoading, 
    error: processesError,
    refetch: refetchProcesses 
  } = useQuery({
    queryKey: ['processes'],
    queryFn: fetchProcesses,
    enabled: health?.flightsql_connected === true,
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
      console.error('Failed to generate trace:', error)
      // TODO: Show error notification
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
    <div className="min-h-screen bg-background">
      <div className="container mx-auto px-4 py-8">
        {/* Header */}
        <div className="mb-8">
          <h1 className="text-4xl font-bold mb-2">Analytics Web App</h1>
          <p className="text-muted-foreground text-lg">
            Explore and analyze micromegas telemetry data with advanced querying and export capabilities
          </p>
        </div>

        {/* Status Cards */}
        <div className="grid grid-cols-1 md:grid-cols-3 gap-6 mb-8">
          <Card>
            <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
              <CardTitle className="text-sm font-medium">System Status</CardTitle>
              <Activity className="h-4 w-4 text-muted-foreground" />
            </CardHeader>
            <CardContent>
              <div className="flex items-center space-x-2">
                <div className={`w-2 h-2 rounded-full ${
                  health?.status === 'healthy' ? 'bg-green-500' : 
                  health?.status === 'degraded' ? 'bg-yellow-500' : 'bg-red-500'
                }`} />
                <span className="font-semibold">
                  {healthLoading ? 'Checking...' : health?.status || 'Unknown'}
                </span>
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
              <CardTitle className="text-sm font-medium">FlightSQL Connection</CardTitle>
              <Database className="h-4 w-4 text-muted-foreground" />
            </CardHeader>
            <CardContent>
              <div className="flex items-center space-x-2">
                <div className={`w-2 h-2 rounded-full ${
                  health?.flightsql_connected ? 'bg-green-500' : 'bg-red-500'
                }`} />
                <span className="font-semibold">
                  {healthLoading ? 'Checking...' : 
                   health?.flightsql_connected ? 'Connected' : 'Disconnected'}
                </span>
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
              <CardTitle className="text-sm font-medium">Available Processes</CardTitle>
              <Zap className="h-4 w-4 text-muted-foreground" />
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold">
                {processesLoading ? '...' : processes.length}
              </div>
            </CardContent>
          </Card>
        </div>

        {/* Action Bar */}
        <div className="flex justify-between items-center mb-6">
          <div>
            {processesError && (
              <div className="flex items-center gap-2 text-red-600">
                <AlertCircle className="w-5 h-5" />
                <span>Failed to load processes</span>
              </div>
            )}
          </div>
          <Button 
            onClick={handleRefresh} 
            disabled={processesLoading}
            variant="outline"
            className="flex items-center gap-2"
          >
            <RefreshCw className={`w-4 h-4 ${processesLoading ? 'animate-spin' : ''}`} />
            Refresh
          </Button>
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
        {!health?.flightsql_connected ? (
          <Card>
            <CardContent className="flex flex-col items-center justify-center py-12">
              <AlertCircle className="w-12 h-12 text-muted-foreground mb-4" />
              <h3 className="text-lg font-semibold mb-2">FlightSQL Connection Required</h3>
              <p className="text-muted-foreground text-center">
                Unable to connect to the FlightSQL server. Please ensure the analytics service is running.
              </p>
            </CardContent>
          </Card>
        ) : processesLoading ? (
          <Card>
            <CardContent className="flex items-center justify-center py-12">
              <div className="animate-spin rounded-full h-8 w-8 border-2 border-primary border-t-transparent mr-4" />
              <span>Loading processes...</span>
            </CardContent>
          </Card>
        ) : (
          <ProcessTable
            processes={processes}
            onGenerateTrace={handleGenerateTrace}
            isGenerating={isGenerating}
          />
        )}
      </div>
    </div>
  )
}