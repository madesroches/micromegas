'use client'

import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useParams } from 'next/navigation'
import { ProcessInfo, ProgressUpdate, GenerateTraceRequest, LogEntry } from '@/types'
import { fetchProcesses, generateTrace, fetchProcessLogEntries } from '@/lib/api'
import { TraceGenerationProgress } from '@/components/TraceGenerationProgress'
import { formatRelativeTime } from '@/lib/utils'
import Link from 'next/link'
import { ArrowLeft, Play, RefreshCw, Filter } from 'lucide-react'

export default function ProcessDetailPage() {
  const params = useParams()
  const processId = params.id as string
  
  const [isGenerating, setIsGenerating] = useState(false)
  const [progress, setProgress] = useState<ProgressUpdate | null>(null)
  const [activeTab, setActiveTab] = useState<'info' | 'trace' | 'logs'>('info')
  const [includeThreadSpans, setIncludeThreadSpans] = useState(true)
  const [includeAsyncSpans, setIncludeAsyncSpans] = useState(true)
  const [logLevel, setLogLevel] = useState<string>('all')
  const [logLimit, setLogLimit] = useState<number>(50)

  // Fetch processes to find the specific process
  const { data: processes = [] } = useQuery({
    queryKey: ['processes'],
    queryFn: fetchProcesses,
  })

  const process = processes.find(p => p.process_id === processId)

  // Fetch log entries
  const { 
    data: logEntries = [], 
    isLoading: logsLoading,
    refetch: refetchLogs 
  } = useQuery({
    queryKey: ['logs', processId, logLevel, logLimit],
    queryFn: () => fetchProcessLogEntries(processId, logLevel, logLimit),
    enabled: activeTab === 'logs' && !!process,
  })

  const handleGenerateTrace = async () => {
    setIsGenerating(true)
    setProgress(null)

    const request: GenerateTraceRequest = {
      include_async_spans: includeAsyncSpans,
      include_thread_spans: includeThreadSpans,
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

  const startTime = new Date(process.begin)
  const endTime = new Date(process.end)
  const duration = Math.round((endTime.getTime() - startTime.getTime()) / 1000)

  return (
    <div className="min-h-screen" style={{ backgroundColor: '#f8fafc' }}>
      {/* Header */}
      <div className="bg-white border-b border-gray-200 shadow-sm">
        <div className="px-8 py-4">
          <div className="max-w-7xl mx-auto flex items-center gap-4">
            <div className="flex-1">
              <h1 className="text-2xl font-semibold text-gray-800">{process.exe} ({process.process_id.substring(0, 11)})</h1>
              <p className="text-sm text-gray-600 mt-1">Generate and configure Perfetto traces for this process</p>
            </div>
          </div>
        </div>
      </div>

      <div className="max-w-7xl mx-auto px-8 py-8">
        {/* Breadcrumb */}
        <div className="flex items-center gap-2 mb-4 text-sm">
          <Link href="/" className="text-blue-600 hover:underline">Analytics</Link>
          <span className="text-gray-400">›</span>
          <Link href="/" className="text-blue-600 hover:underline">Processes</Link>
          <span className="text-gray-400">›</span>
          <span className="text-gray-600">{process.exe}</span>
        </div>

        {/* Tab Navigation */}
        <div className="flex mb-8">
          {[
            { id: 'info' as const, label: 'Process Info' },
            { id: 'trace' as const, label: 'Generate Trace' },
            { id: 'logs' as const, label: 'Recent Logs' },
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
              <div className="grid grid-cols-1 md:grid-cols-3 gap-6 mb-6">
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Start Time</div>
                  <div className="text-2xl font-bold text-gray-800">
                    {startTime.toLocaleTimeString('en-US', { 
                      hour: 'numeric', 
                      minute: '2-digit', 
                      hour12: true 
                    })}
                  </div>
                  <div className="text-sm text-gray-600">
                    {startTime.toLocaleDateString('en-US', { 
                      month: 'short', 
                      day: 'numeric',
                      year: 'numeric'
                    })}
                  </div>
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Last Update</div>
                  <div className="text-2xl font-bold text-gray-800">
                    {endTime.toLocaleTimeString('en-US', { 
                      hour: 'numeric', 
                      minute: '2-digit', 
                      hour12: true 
                    })}
                  </div>
                  <div className="text-sm text-gray-600">{formatRelativeTime(process.end)}</div>
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Threads</div>
                  <div className="text-2xl font-bold text-gray-800">8</div>
                  <div className="text-sm text-gray-600">Number of thread streams</div>
                </div>
              </div>

              {/* Metrics Grid */}
              <div className="grid grid-cols-1 md:grid-cols-3 gap-6 mb-6">
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Log Entries</div>
                  <div className="text-2xl font-bold text-gray-800">12,456</div>
                  <div className="text-sm text-gray-600">Number of log entries</div>
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Measures</div>
                  <div className="text-2xl font-bold text-gray-800">834</div>
                  <div className="text-sm text-gray-600">Number of measures</div>
                </div>
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Trace Events</div>
                  <div className="text-2xl font-bold text-gray-800">5,137</div>
                  <div className="text-sm text-gray-600">Number of trace events</div>
                </div>
              </div>

              {/* Properties Grid */}
              <div className="grid grid-cols-1 md:grid-cols-4 gap-6">
                <div className="bg-gray-50 rounded p-4">
                  <div className="text-xs text-gray-600 uppercase font-medium mb-1">Process ID</div>
                  <div className="text-xl font-bold text-gray-800">{process.process_id.substring(0, 11)}</div>
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
                    <div className="flex justify-between">
                      <span className="text-gray-600">distro:</span>
                      <span className="text-gray-800 font-semibold">{process.distro}</span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-gray-600">duration:</span>
                      <span className="text-gray-800 font-semibold">
                        {duration < 60 ? `${duration}s` : 
                         duration < 3600 ? `${Math.floor(duration / 60)}m` :
                         `${Math.floor(duration / 3600)}h`}
                      </span>
                    </div>
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
                  <label className="block text-sm font-medium text-gray-700 mb-2">Time Range</label>
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                    <input 
                      type="datetime-local" 
                      className="px-3 py-2 border border-gray-300 rounded text-sm focus:outline-none focus:ring-2 focus:ring-blue-500" 
                      defaultValue={startTime.toISOString().slice(0, 16)}
                    />
                    <input 
                      type="datetime-local" 
                      className="px-3 py-2 border border-gray-300 rounded text-sm focus:outline-none focus:ring-2 focus:ring-blue-500" 
                      defaultValue={endTime.toISOString().slice(0, 16)}
                    />
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
                    disabled={isGenerating}
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

          {activeTab === 'logs' && (
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
                <button 
                  className="px-3 py-1 text-xs bg-gray-100 border border-gray-300 rounded hover:bg-gray-200 transition-colors"
                  onClick={() => refetchLogs()}
                >
                  Refresh
                </button>
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
                        <div className="text-gray-500 flex-shrink-0 font-medium">{timeStr}</div>
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