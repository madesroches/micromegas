'use client'

import { useState, useMemo } from 'react'
import { ProcessInfo } from '@/types'
import { formatDateTime, formatRelativeTime, cn } from '@/lib/utils'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Play, Search, Filter } from 'lucide-react'

interface ProcessTableProps {
  processes: ProcessInfo[]
  onGenerateTrace: (processId: string) => void
  isGenerating: boolean
}

export function ProcessTable({ processes, onGenerateTrace, isGenerating }: ProcessTableProps) {
  const [searchTerm, setSearchTerm] = useState('')
  const [sortField, setSortField] = useState<keyof ProcessInfo>('begin')
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
      
      if (sortField === 'begin' || sortField === 'end') {
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
    <div className="space-y-4">
      {/* Search and Filter */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Search className="w-5 h-5" />
            Search & Filter
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="flex gap-4">
            <div className="flex-1">
              <input
                type="text"
                placeholder="Search by executable, computer, username, or process ID..."
                value={searchTerm}
                onChange={(e) => setSearchTerm(e.target.value)}
                className="w-full px-3 py-2 border border-input rounded-md bg-background"
              />
            </div>
            <Button variant="outline" size="sm">
              <Filter className="w-4 h-4 mr-2" />
              Filters
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* Process Table */}
      <Card>
        <CardHeader>
          <CardTitle>Available Processes ({filteredAndSortedProcesses.length})</CardTitle>
        </CardHeader>
        <CardContent className="p-0">
          <div className="overflow-x-auto">
            <table className="w-full">
              <thead>
                <tr className="border-b bg-muted/50">
                  <th className="p-3 text-left">
                    <Button 
                      variant="ghost" 
                      size="sm"
                      onClick={() => handleSort('exe')}
                      className="font-semibold"
                    >
                      Executable
                      {sortField === 'exe' && (
                        <span className="ml-1">{sortDirection === 'asc' ? '↑' : '↓'}</span>
                      )}
                    </Button>
                  </th>
                  <th className="p-3 text-left">
                    <Button 
                      variant="ghost" 
                      size="sm"
                      onClick={() => handleSort('computer')}
                      className="font-semibold"
                    >
                      Computer
                      {sortField === 'computer' && (
                        <span className="ml-1">{sortDirection === 'asc' ? '↑' : '↓'}</span>
                      )}
                    </Button>
                  </th>
                  <th className="p-3 text-left">
                    <Button 
                      variant="ghost" 
                      size="sm"
                      onClick={() => handleSort('username')}
                      className="font-semibold"
                    >
                      User
                      {sortField === 'username' && (
                        <span className="ml-1">{sortDirection === 'asc' ? '↑' : '↓'}</span>
                      )}
                    </Button>
                  </th>
                  <th className="p-3 text-left">
                    <Button 
                      variant="ghost" 
                      size="sm"
                      onClick={() => handleSort('begin')}
                      className="font-semibold"
                    >
                      Start Time
                      {sortField === 'begin' && (
                        <span className="ml-1">{sortDirection === 'asc' ? '↑' : '↓'}</span>
                      )}
                    </Button>
                  </th>
                  <th className="p-3 text-left">Duration</th>
                  <th className="p-3 text-left">Actions</th>
                </tr>
              </thead>
              <tbody>
                {filteredAndSortedProcesses.map((process) => {
                  const startTime = new Date(process.begin)
                  const endTime = new Date(process.end)
                  const duration = Math.round((endTime.getTime() - startTime.getTime()) / 1000)
                  
                  return (
                    <tr key={process.process_id} className="border-b hover:bg-muted/50">
                      <td className="p-3">
                        <div>
                          <div className="font-medium">{process.exe}</div>
                          <div className="text-sm text-muted-foreground">
                            {process.process_id.substring(0, 8)}...
                          </div>
                        </div>
                      </td>
                      <td className="p-3">
                        <div>
                          <div className="font-medium">{process.computer}</div>
                          <div className="text-sm text-muted-foreground">{process.distro}</div>
                        </div>
                      </td>
                      <td className="p-3">{process.username}</td>
                      <td className="p-3">
                        <div>
                          <div className="font-medium">{formatRelativeTime(process.begin)}</div>
                          <div className="text-sm text-muted-foreground">
                            {formatDateTime(process.begin)}
                          </div>
                        </div>
                      </td>
                      <td className="p-3">
                        <div className="text-sm">
                          {duration < 60 ? `${duration}s` : 
                           duration < 3600 ? `${Math.floor(duration / 60)}m ${duration % 60}s` :
                           `${Math.floor(duration / 3600)}h ${Math.floor((duration % 3600) / 60)}m`}
                        </div>
                      </td>
                      <td className="p-3">
                        <Button
                          size="sm"
                          onClick={() => onGenerateTrace(process.process_id)}
                          disabled={isGenerating}
                          className="flex items-center gap-2"
                        >
                          <Play className="w-4 h-4" />
                          Analyze
                        </Button>
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
          
          {filteredAndSortedProcesses.length === 0 && (
            <div className="p-8 text-center text-muted-foreground">
              {searchTerm ? 'No processes match your search criteria.' : 'No processes available.'}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}