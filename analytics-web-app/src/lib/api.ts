import { ProcessInfo, TraceMetadata, GenerateTraceRequest, HealthCheck, ProgressUpdate, BinaryStartMarker, LogEntry } from '@/types'

const API_BASE = process.env.NODE_ENV === 'development' ? 'http://localhost:8001/api' : '/api'

export async function fetchProcesses(): Promise<ProcessInfo[]> {
  const response = await fetch(`${API_BASE}/processes`)
  if (!response.ok) {
    throw new Error('Failed to fetch processes')
  }
  return response.json()
}

export async function fetchTraceMetadata(processId: string): Promise<TraceMetadata> {
  const response = await fetch(`${API_BASE}/perfetto/${processId}/info`)
  if (!response.ok) {
    throw new Error('Failed to fetch trace metadata')
  }
  return response.json()
}

export async function validateTrace(processId: string): Promise<any> {
  const response = await fetch(`${API_BASE}/perfetto/${processId}/validate`, {
    method: 'POST'
  })
  if (!response.ok) {
    throw new Error('Failed to validate trace')
  }
  return response.json()
}

export async function fetchHealthCheck(): Promise<HealthCheck> {
  const response = await fetch(`${API_BASE}/health`)
  if (!response.ok) {
    throw new Error('Failed to fetch health status')
  }
  return response.json()
}

export async function generateTrace(
  processId: string, 
  request: GenerateTraceRequest,
  onProgress?: (update: ProgressUpdate) => void
): Promise<void> {
  const response = await fetch(`${API_BASE}/perfetto/${processId}/generate`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request)
  })
  
  if (!response.ok) {
    throw new Error('Failed to generate trace')
  }
  
  if (!response.body) {
    throw new Error('No response body')
  }
  
  const reader = response.body.getReader()
  const chunks: Uint8Array[] = []
  let progressComplete = false
  
  while (true) {
    const { done, value } = await reader.read()
    if (done) break
    
    if (!progressComplete) {
      // Try to parse as JSON progress update
      try {
        const chunk = new TextDecoder().decode(value)
        const lines = chunk.split('\n').filter(line => line.trim())
        
        for (const line of lines) {
          try {
            const update = JSON.parse(line) as ProgressUpdate | BinaryStartMarker
            
            if (update.type === 'progress' && onProgress) {
              onProgress(update as ProgressUpdate)
              continue
            } else if (update.type === 'binary_start') {
              progressComplete = true
              break
            }
          } catch {
            // Not JSON, must be binary data
            progressComplete = true
            chunks.push(value)
            break
          }
        }
        
        if (progressComplete && chunks.length === 0) continue
      } catch {
        // Not JSON, must be binary data
        progressComplete = true
      }
    }
    
    if (progressComplete) {
      // Collect binary chunks
      chunks.push(value)
    }
  }
  
  // Create blob and download
  const blob = new Blob(chunks, { type: 'application/octet-stream' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `perfetto-${processId}-${Date.now()}.pb`
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}

export async function fetchProcessLogEntries(
  processId: string, 
  level?: string, 
  limit: number = 50
): Promise<LogEntry[]> {
  const params = new URLSearchParams()
  if (level && level !== 'all') {
    params.append('level', level.toLowerCase())
  }
  params.append('limit', limit.toString())
  
  const response = await fetch(`${API_BASE}/process/${processId}/log-entries?${params}`)
  if (!response.ok) {
    throw new Error('Failed to fetch log entries')
  }
  return response.json()
}