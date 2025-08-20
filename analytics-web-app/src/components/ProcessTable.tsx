'use client'

import { useState, useMemo } from 'react'
import { ProcessInfo } from '@/types'
import { CopyableProcessId } from '@/components/CopyableProcessId'
import { formatRelativeTime } from '@/lib/utils'
import Link from 'next/link'

interface ProcessTableProps {
  processes: ProcessInfo[]
  onGenerateTrace: (processId: string) => void
  isGenerating: boolean
  onRefresh?: () => void
}

export function ProcessTable({ processes, onGenerateTrace, isGenerating, onRefresh }: ProcessTableProps) {
  const [searchTerm, setSearchTerm] = useState('')
  const [sortField, setSortField] = useState<keyof ProcessInfo>('last_update_time')
  const [sortDirection, setSortDirection] = useState<'asc' | 'desc'>('desc')

  const filteredAndSortedProcesses = useMemo(() => {
    const filtered = processes.filter(process => 
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

  const handleSort = (field: keyof ProcessInfo) => {
    if (field === sortField) {
      setSortDirection(sortDirection === 'asc' ? 'desc' : 'asc')
    } else {
      setSortField(field)
      setSortDirection('desc')
    }
  }

  return (
    <div className="space-y-8">
      {/* Process Filters */}
      <div className="bg-white rounded-lg border border-gray-200 shadow-sm p-6">
        <h2 className="text-lg font-semibold text-gray-800 mb-4">Process Filters</h2>
        <input
          type="text"
          placeholder="Search processes by name, process_id, or command..."
          value={searchTerm}
          onChange={(e) => setSearchTerm(e.target.value)}
          className="w-full px-3 py-2 border border-gray-300 rounded-md text-sm mb-4 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-blue-500 transition-all"
        />
        
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-4">
            <div className="flex items-center gap-2">
              <label className="text-sm font-medium text-gray-700">Time Range:</label>
              <select className="px-2 py-1 border border-gray-300 rounded text-sm bg-white" defaultValue="Last 24 Hours">
                <option>Last Hour</option>
                <option>Last 6 Hours</option>
                <option>Last 24 Hours</option>
                <option>Last Week</option>
                <option>Custom Range</option>
              </select>
            </div>
          </div>
          <button 
            onClick={onRefresh}
            className="flex items-center gap-2 px-4 py-2 bg-blue-600 text-white rounded text-sm font-medium hover:bg-blue-700 transition-colors"
          >
            🔄 Refresh
          </button>
        </div>
      </div>

      {/* Process Table */}
      <div className="bg-white rounded-lg border border-gray-200 shadow-sm overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="bg-gray-50 border-b border-gray-200">
                <th className="px-4 py-3 text-left font-semibold text-gray-700 whitespace-nowrap">Process</th>
                <th className="px-4 py-3 text-left font-semibold text-gray-700 whitespace-nowrap">Process ID</th>
                <th className="px-4 py-3 text-left font-semibold text-gray-700 whitespace-nowrap">Start Time</th>
                <th className="px-4 py-3 text-left font-semibold text-gray-700 whitespace-nowrap">Last Update</th>
                <th className="px-4 py-3 text-left font-semibold text-gray-700 whitespace-nowrap">Username</th>
                <th className="px-4 py-3 text-left font-semibold text-gray-700 whitespace-nowrap">Computer</th>
              </tr>
            </thead>
            <tbody>
              {filteredAndSortedProcesses.map((process) => {
                const startTime = new Date(process.start_time)
                const lastUpdateTime = new Date(process.last_update_time)
                
                return (
                  <tr key={process.process_id} className="border-b border-gray-100 hover:bg-gray-50">
                    <td className="px-4 py-3">
                      <Link 
                        href={`/process/${process.process_id}`}
                        className="text-blue-600 hover:text-blue-800 hover:underline font-semibold"
                      >
                        {process.exe}
                      </Link>
                      <div className="text-xs text-gray-500 mt-0.5">{process.exe}</div>
                    </td>
                    <td className="px-4 py-3">
                      <CopyableProcessId processId={process.process_id} truncate={true} className="text-sm" />
                    </td>
                    <td className="px-4 py-3">
                      <div className="font-medium text-gray-700">
                        {startTime.toLocaleTimeString('en-US', { 
                          hour: 'numeric', 
                          minute: '2-digit', 
                          hour12: true 
                        })}
                      </div>
                      <div className="text-xs text-gray-500">
                        {startTime.toLocaleDateString('en-US', { 
                          month: 'short', 
                          day: 'numeric' 
                        })}
                      </div>
                    </td>
                    <td className="px-4 py-3">
                      <div className="font-medium text-gray-700">
                        {lastUpdateTime.toLocaleTimeString('en-US', { 
                          hour: 'numeric', 
                          minute: '2-digit', 
                          hour12: true 
                        })}
                      </div>
                      <div className="text-xs text-gray-500">{formatRelativeTime(process.last_update_time)}</div>
                    </td>
                    <td className="px-4 py-3 text-gray-700">{process.username}</td>
                    <td className="px-4 py-3 text-gray-700">{process.computer}</td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
        
        {filteredAndSortedProcesses.length === 0 && (
          <div className="px-4 py-8 text-center text-gray-500">
            {searchTerm ? 'No processes match your search criteria.' : 'No processes available.'}
          </div>
        )}
      </div>
    </div>
  )
}