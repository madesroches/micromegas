'use client'

import { useState, useEffect } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useParams } from 'next/navigation'
import { ProgressUpdate, GenerateTraceRequest } from '@/types'
import { fetchProcesses, generateTrace, fetchProcessLogEntries, fetchProcessStatistics } from '@/lib/api'
import { TraceGenerationProgress } from '@/components/TraceGenerationProgress'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { PreciseTimestamp } from '@/components/PreciseTimestamp'
import Link from 'next/link'
import { Play, RefreshCw } from 'lucide-react'

export default function ProcessDetailPage() {
  const params = useParams()
  const processId = params.id as string
  
  const [isGenerating, setIsGenerating] = useState(false)
  const [progress, setProgress] = useState<ProgressUpdate | null>(null)
  const [activeTab, setActiveTab] = useState<'info' | 'trace' | 'log'>('info')
  const [includeThreadSpans, setIncludeThreadSpans] = useState(true)
  const [includeAsyncSpans, setIncludeAsyncSpans] = useState(true)
  const [logLevel, setLogLevel] = useState<string>('all')
  const [logLimit, setLogLimit] = useState<number>(50)
  const [traceStartTime, setTraceStartTime] = useState<string>('')
  const [traceEndTime, setTraceEndTime] = useState<string>('')

  // Fetch processes to find the specific process
  const { 
    data: processes = [], 
    refetch: refetchProcesses 
  } = useQuery({
    queryKey: ['processes'],
    queryFn: fetchProcesses,
  })

  const process = processes.find(p => p.process_id === processId)

  // Set default time range values when process is loaded (use full RFC3339 strings)
  useEffect(() => {
    if (process && !traceStartTime && !traceEndTime) {
      setTraceStartTime(process.start_time) // Keep full RFC3339 format with nanoseconds
      setTraceEndTime(process.last_update_time)
    }
  }, [process, traceStartTime, traceEndTime])

  // Helper to format RFC3339 string for display
  const formatDisplayTime = (rfc3339: string): string => {
    try {
      const date = new Date(rfc3339)
      if (isNaN(date.getTime())) return "Invalid timestamp"
      return date.toLocaleString('en-US', {
        year: 'numeric',
        month: '2-digit', 
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
        second: '2-digit',
        hour12: false
      }) + ` (${rfc3339.split('T')[1]})`
    } catch {
      return "Invalid timestamp format"
    }
  }

  // Validate RFC3339 timestamp
  const isValidRFC3339 = (timestamp: string): boolean => {
    try {
      const date = new Date(timestamp)
      return !isNaN(date.getTime()) && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z$/.test(timestamp)
    } catch {
      return false
    }
  }

  // Fetch log entries
  const { 
    data: logEntries = [], 
    isLoading: logsLoading,
    refetch: refetchLogs 
  } = useQuery({
    queryKey: ['logs', processId, logLevel, logLimit],
    queryFn: () => fetchProcessLogEntries(processId, logLevel, logLimit),
    enabled: activeTab === 'log' && !!process,
    staleTime: 0, // Always consider data stale so it refetches when tab is activated
  })

  // Fetch process statistics
  const { 
    data: statistics, 
    refetch: refetchStatistics 
  } = useQuery({
    queryKey: ['statistics', processId],
    queryFn: () => fetchProcessStatistics(processId),
    enabled: !!process,
    staleTime: 0, // Always consider data stale so it refetches on refresh
  })

  const handleGenerateTrace = async () => {
    setIsGenerating(true)
    setProgress(null)

    const request: GenerateTraceRequest = {
      include_async_spans: includeAsyncSpans,
      include_thread_spans: includeThreadSpans,
      time_range: traceStartTime && traceEndTime ? {
        begin: traceStartTime, // Already RFC3339 format
        end: traceEndTime
      } : undefined,
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

  const handleRefresh = () => {
    refetchProcesses()
    refetchStatistics()
    if (activeTab === 'log') {
      refetchLogs()
    }
  }

  if (!process) {
    return (
      <div className="min-h-screen" style={{ backgroundColor: '#f8fafc' }}>
        <div className="bg-white border-b border-gray-200 shadow-sm">
          <div className="px-8 py-4">
            <h1 className="text-2xl font-semibold text-gray-800">Process Not Found</h1>
            <p className="text-sm text-gray-600 mt-1">The requested process could not be found</p>
          </div>
        </div>
        <div className="max-w-7xl mx-auto px-8 py-8">
          <Link href="/" className="text-blue-600 hover:underline">← Back to Analytics</Link>
        </div>
      </div>
    )
  }


  // Format precise duration with nanosecond accuracy
  const formatPreciseDuration = (startTime: string, endTime: string): string => {
    try {
      const start = new Date(startTime)
      const end = new Date(endTime)
      const diffMs = end.getTime() - start.getTime()

      if (diffMs < 0) return "Invalid"

      // Extract nanosecond precision from RFC3339 strings
      const extractNanos = (timestamp: string): number => {
        const match = timestamp.match(/\.(\d+)Z?$/)
        if (!match) return 0
        const fractionalSeconds = match[1].padEnd(9, '0').slice(0, 9)
        return parseInt(fractionalSeconds)
      }

      const startNanos = extractNanos(startTime)
      const endNanos = extractNanos(endTime)
      const nanosDiff = endNanos - startNanos
      const totalMs = diffMs + (nanosDiff / 1000000)

      // Convert to appropriate units
      const totalSeconds = Math.floor(totalMs / 1000)
      const remainingMs = Math.floor(totalMs % 1000)
      const remainingMicros = Math.floor((nanosDiff % 1000000) / 1000)

      const minutes = Math.floor(totalSeconds / 60)
      const seconds = totalSeconds % 60
      const hours = Math.floor(minutes / 60)
      const mins = minutes % 60
      const days = Math.floor(hours / 24)
      const hrs = hours % 24

      if (days > 0) {
        return `${days}d ${hrs}h ${mins}m`
      } else if (hours > 0) {
        return `${hours}h ${mins}m ${seconds}s`
      } else if (minutes > 0) {
        return `${minutes}m ${seconds}.${remainingMs.toString().padStart(3, '0')}s`
      } else {
        return `${seconds}.${remainingMs.toString().padStart(3, '0')}.${remainingMicros.toString().padStart(3, '0')}s`
      }
    } catch {
      return "Invalid"
    }
  }

  return (
    <div className="min-h-screen" style={{ backgroundColor: '#f8fafc' }}>
      {/* Header */}
      <div className="bg-white border-b border-gray-200 shadow-sm">
        <div className="px-8 py-4">
          <div className="max-w-7xl mx-auto flex items-center gap-4">
            <div className="flex-1">
              <h1 className="text-2xl font-semibold text-gray-800">Process Details</h1>
              <p className="text-sm text-gray-600 mt-1">
                {process.exe} (<CopyableProcessId processId={process.process_id} className="text-sm" />)
              </p>
            </div>
            <button
              onClick={handleRefresh}
              className="flex items-center gap-2 px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 transition-colors text-sm"
            >
              <RefreshCw className="w-4 h-4" />
              Refresh
            </button>
          </div>
        </div>
      </div>

      <div className="max-w-7xl mx-auto px-8 py-8">
        {/* Breadcrumb */}
        <div className="flex items-center gap-2 mb-4 text-sm">
          <Link href="/" className="text-blue-600 hover:underline">Process Explorer</Link>
          <span className="text-gray-400">›</span>
          <span className="text-gray-600">{process.exe}</span>
        </div>

        {/* Tab Navigation */}
        <div className="flex mb-8">
          {[
            { id: 'info' as const, label: 'Process Info' },
            { id: 'trace' as const, label: 'Generate Trace' },
            { id: 'log' as const, label: 'Recent Log' },
          ].map((tab) => (
            <button
              key={tab.id}
              onClick={() => setActiveTab(tab.id)}
              className={`px-6 py-3 text-sm font-medium border border-gray-200 border-b-0 rounded-t-lg mr-1 transition-all ${
                activeTab === tab.id
                  ? 'bg-white text-blue-600 border-b border-white -mb-px relative z-10'
                  : 'bg-gray-50 text-gray-600 hover:bg-gray-100 hover:text-gray-700'
              }`}
            >
              {tab.label}
            </button>
          ))}
        </div>

        {/* Tab Content */}
        <div className="bg-white rounded-b-lg rounded-tr-lg border border-gray-200 shadow-sm border-t-0 p-6">
          {activeTab === 'info' && (
            <div>
              <h2 className="text-lg font-semibold text-gray-800 mb-6">Process Information</h2>
              
              {/* Overview Grid */}
              <div className="grid grid-cols-1 md:grid-cols-4 gap-6 mb-6">
                <div className="bg-gray-50 rounded p-4">
                  <PreciseTimestamp 
                    timestamp={process.start_time} 
                    label="Start Time" 
                    showDuration={false}
                    className="" 
                  />
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <PreciseTimestamp 
                    timestamp={process.last_update_time} 
                    label="Last Update" 
                    showDuration={true}
                    durationStart={process.start_time}
                    className="" 
                  />
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Process Duration</div>
                  <div className="text-2xl font-bold text-gray-800">{formatPreciseDuration(process.start_time, process.last_update_time)}</div>
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Active Threads</div>
                  <div className="text-2xl font-bold text-gray-800">{statistics?.thread_count || 0}</div>
                  <div className="text-sm text-gray-600">Number of thread streams</div>
                </div>
              </div>

              {/* Metrics Grid */}
              <div className="grid grid-cols-1 md:grid-cols-3 gap-6 mb-6">
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Log Entries</div>
                  <div className="text-2xl font-bold text-gray-800">{statistics?.log_entries?.toLocaleString() || 0}</div>
                  <div className="text-sm text-gray-600">Number of log entries</div>
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Measures</div>
                  <div className="text-2xl font-bold text-gray-800">{statistics?.measures?.toLocaleString() || 0}</div>
                  <div className="text-sm text-gray-600">Number of measures</div>
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Trace Events</div>
                  <div className="text-2xl font-bold text-gray-800">{statistics?.trace_events?.toLocaleString() || 0}</div>
                  <div className="text-sm text-gray-600">Number of trace events</div>
                </div>
              </div>

              {/* Properties Grid */}
              <div className="grid grid-cols-1 md:grid-cols-4 gap-6">
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Process ID</div>
                  <div className="text-xl font-bold text-gray-800">
                    <CopyableProcessId processId={process.process_id} className="text-xl font-bold" />
                  </div>
                  <div className="text-sm text-gray-600">Unique process identifier</div>
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Username</div>
                  <div className="text-xl font-bold text-gray-800">{process.username}</div>
                  <div className="text-sm text-gray-600">Process owner</div>
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Computer</div>
                  <div className="text-xl font-bold text-gray-800">{process.computer}</div>
                  <div className="text-sm text-gray-600">Host machine</div>
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Properties</div>
                  <div className="text-xs space-y-1">
                    {Object.entries(process.properties).length > 0 ? (
                      Object.entries(process.properties).map(([key, value]) => (
                        <div key={key} className="flex justify-between">
                          <span className="text-gray-600">{key}:</span>
                          <span className="text-gray-800 font-semibold">{value}</span>
                        </div>
                      ))
                    ) : (
                      <div className="text-gray-500 italic">No properties available</div>
                    )}
                  </div>
                </div>
              </div>
            </div>
          )}

          {activeTab === 'trace' && (
            <div>
              <h2 className="text-lg font-semibold text-gray-800 mb-6">Generate Perfetto Trace</h2>
              
              <div className="space-y-6">
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-4">Span Types to Include</label>
                  <div className="space-y-3">
                    <div 
                      className={`flex items-center gap-3 p-3 border rounded cursor-pointer transition-all ${
                        includeThreadSpans ? 'bg-blue-50 border-blue-500' : 'border-gray-200 hover:bg-gray-50'
                      }`}
                      onClick={() => setIncludeThreadSpans(!includeThreadSpans)}
                    >
                      <input 
                        type="checkbox" 
                        checked={includeThreadSpans}
                        onChange={() => setIncludeThreadSpans(!includeThreadSpans)}
                        className="w-4 h-4"
                      />
                      <div className="flex-1 text-sm text-gray-700">Thread Events</div>
                      <div className="text-xs text-gray-600 font-medium">1,245</div>
                    </div>
                    <div 
                      className={`flex items-center gap-3 p-3 border rounded cursor-pointer transition-all ${
                        includeAsyncSpans ? 'bg-blue-50 border-blue-500' : 'border-gray-200 hover:bg-gray-50'
                      }`}
                      onClick={() => setIncludeAsyncSpans(!includeAsyncSpans)}
                    >
                      <input 
                        type="checkbox" 
                        checked={includeAsyncSpans}
                        onChange={() => setIncludeAsyncSpans(!includeAsyncSpans)}
                        className="w-4 h-4"
                      />
                      <div className="flex-1 text-sm text-gray-700">Async Span Events</div>
                      <div className="text-xs text-gray-600 font-medium">3,892</div>
                    </div>
                  </div>
                </div>

                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-2">
                    Time Range (RFC3339 format with nanosecond precision)
                  </label>
                  <div className="space-y-3">
                    <div>
                      <label className="block text-xs font-medium text-gray-600 mb-1">Start Time</label>
                      <div className="flex items-center gap-2">
                        <input 
                          type="text"
                          value={traceStartTime}
                          onChange={(e) => setTraceStartTime(e.target.value)}
                          placeholder="2025-08-20T15:26:02.479554123Z"
                          className={`flex-1 px-3 py-2 border rounded text-sm font-mono focus:outline-none focus:ring-2 ${
                            traceStartTime && !isValidRFC3339(traceStartTime) 
                              ? 'border-red-300 focus:ring-red-500' 
                              : 'border-gray-300 focus:ring-blue-500'
                          }`} 
                        />
                        <button
                          type="button"
                          onClick={() => process && setTraceStartTime(process.start_time)}
                          className="px-3 py-2 bg-gray-100 hover:bg-gray-200 text-gray-700 rounded text-xs transition-colors"
                        >
                          Reset
                        </button>
                      </div>
                      {traceStartTime && (
                        <div className="mt-1 text-xs text-gray-500">
                          Display: {formatDisplayTime(traceStartTime)}
                        </div>
                      )}
                    </div>
                    
                    <div>
                      <label className="block text-xs font-medium text-gray-600 mb-1">End Time</label>
                      <div className="flex items-center gap-2">
                        <input 
                          type="text"
                          value={traceEndTime}
                          onChange={(e) => setTraceEndTime(e.target.value)}
                          placeholder="2025-08-20T19:47:04.538264789Z"
                          className={`flex-1 px-3 py-2 border rounded text-sm font-mono focus:outline-none focus:ring-2 ${
                            traceEndTime && !isValidRFC3339(traceEndTime) 
                              ? 'border-red-300 focus:ring-red-500' 
                              : 'border-gray-300 focus:ring-blue-500'
                          }`} 
                        />
                        <button
                          type="button"
                          onClick={() => process && setTraceEndTime(process.last_update_time)}
                          className="px-3 py-2 bg-gray-100 hover:bg-gray-200 text-gray-700 rounded text-xs transition-colors"
                        >
                          Reset
                        </button>
                      </div>
                      {traceEndTime && (
                        <div className="mt-1 text-xs text-gray-500">
                          Display: {formatDisplayTime(traceEndTime)}
                        </div>
                      )}
                    </div>
                  </div>
                </div>

                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-2">Trace Name</label>
                  <input 
                    type="text" 
                    className="w-full px-3 py-2 border border-gray-300 rounded text-sm focus:outline-none focus:ring-2 focus:ring-blue-500" 
                    defaultValue={`${process.exe}-trace-${new Date().toISOString().slice(0, 10).replace(/-/g, '')}`}
                    placeholder="Enter trace name"
                  />
                </div>

                <div>
                  <button 
                    onClick={handleGenerateTrace}
                    disabled={isGenerating || (!!traceStartTime && !isValidRFC3339(traceStartTime)) || (!!traceEndTime && !isValidRFC3339(traceEndTime))}
                    className="w-full flex items-center justify-center gap-2 px-6 py-3 bg-green-600 text-white rounded font-medium hover:bg-green-700 disabled:bg-gray-400 disabled:cursor-not-allowed transition-colors"
                  >
                    <Play className="w-4 h-4" />
                    Generate Perfetto Trace
                  </button>
                </div>

                {/* Generation Progress */}
                {(isGenerating || progress) && (
                  <div className="mt-8">
                    <TraceGenerationProgress
                      isGenerating={isGenerating}
                      progress={progress}
                      processId={processId}
                    />
                  </div>
                )}
              </div>
            </div>
          )}

          {activeTab === 'log' && (
            <div>
              <h2 className="text-lg font-semibold text-gray-800 mb-6">Recent Log Entries</h2>
              
              <div className="flex items-center gap-2 mb-4">
                <select 
                  className="px-2 py-1 text-xs border border-gray-300 rounded bg-white"
                  value={logLevel}
                  onChange={(e) => setLogLevel(e.target.value)}
                >
                  <option value="all">All Levels</option>
                  <option value="fatal">Fatal</option>
                  <option value="error">Error</option>
                  <option value="warn">Warn</option>
                  <option value="info">Info</option>
                  <option value="debug">Debug</option>
                  <option value="trace">Trace</option>
                </select>
                <select 
                  className="px-2 py-1 text-xs border border-gray-300 rounded bg-white"
                  value={logLimit}
                  onChange={(e) => setLogLimit(Number(e.target.value))}
                >
                  <option value={50}>Last 50</option>
                  <option value={100}>Last 100</option>
                  <option value={200}>Last 200</option>
                  <option value={500}>Last 500</option>
                </select>
                <span className="ml-auto text-xs text-gray-600">
                  {logsLoading ? 'Loading...' : `Showing ${logEntries.length} entries`}
                </span>
              </div>

              <div className="bg-gray-900 rounded p-4 font-mono text-sm text-gray-200 max-h-96 overflow-y-auto">
                {logsLoading ? (
                  <div className="text-center text-gray-400 py-4">Loading log entries...</div>
                ) : logEntries.length === 0 ? (
                  <div className="text-center text-gray-400 py-4">No log entries found</div>
                ) : (
                  logEntries.map((log, index) => {
                    const logTime = new Date(log.time)
                    const timeStr = logTime.toLocaleTimeString('en-US', { 
                      hour12: false,
                      hour: '2-digit',
                      minute: '2-digit',
                      second: '2-digit'
                    })
                    
                    // Extract additional precision from RFC3339 string
                    const extractMicroseconds = (timestamp: string): string => {
                      const match = timestamp.match(/\.(\d+)Z?$/)
                      if (!match) return ''
                      const fractional = match[1].padEnd(6, '0').slice(0, 6) // Get microseconds
                      return fractional.slice(3) // Return the microsecond part (after milliseconds)
                    }
                    
                    const microseconds = extractMicroseconds(log.time)
                    const preciseTimeStr = microseconds ? `${timeStr}.${microseconds}` : timeStr
                    
                    const levelColor = {
                      'FATAL': 'text-red-600',
                      'ERROR': 'text-red-400',
                      'WARN': 'text-yellow-400',
                      'INFO': 'text-blue-400',
                      'DEBUG': 'text-gray-400',
                      'TRACE': 'text-gray-500',
                    }[log.level] || 'text-gray-300'
                    
                    return (
                      <div key={index} className="flex gap-3 mb-2 py-1 hover:bg-gray-800 hover:bg-opacity-50 rounded px-1">
                        <div className="text-gray-500 flex-shrink-0 font-medium font-mono text-xs" style={{minWidth: '120px'}}>{preciseTimeStr}</div>
                        <div className={`flex-shrink-0 font-semibold w-14 ${levelColor}`}>{log.level}</div>
                        <div className="text-gray-400 flex-shrink-0 w-32 text-xs font-medium truncate" title={log.target}>
                          {log.target}
                        </div>
                        <div className="text-gray-200 flex-1 break-words">{log.msg}</div>
                      </div>
                    )
                  })
                )}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}