import { streamQuery } from './arrow-stream'

export function triggerTraceDownload(buffer: ArrayBuffer, processId: string): void {
  const blob = new Blob([buffer], { type: 'application/octet-stream' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `trace-${processId}.pb`
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}

export interface FetchPerfettoTraceOptions {
  processId: string
  spanType: 'thread' | 'async' | 'both'
  timeRange: { begin: string; end: string }
  onProgress?: (message: string) => void
  signal?: AbortSignal
  dataSource?: string
}

export async function fetchPerfettoTrace(
  options: FetchPerfettoTraceOptions
): Promise<ArrayBuffer> {
  const { processId, spanType, timeRange, onProgress, signal, dataSource } = options

  const sql = `SELECT chunk_id, chunk_data FROM perfetto_trace_chunks('${processId}', '${spanType}', TIMESTAMP '${timeRange.begin}', TIMESTAMP '${timeRange.end}')`

  const chunks: Uint8Array[] = []
  let totalBytes = 0
  let expectedChunkId = 0

  for await (const result of streamQuery({ sql, begin: timeRange.begin, end: timeRange.end, dataSource }, signal)) {
    if (signal?.aborted) {
      throw new Error(signal.reason ?? 'Trace fetch aborted')
    }
    switch (result.type) {
      case 'batch': {
        const chunkIdCol = result.batch.getChild('chunk_id')
        const chunkDataCol = result.batch.getChild('chunk_data')
        if (!chunkDataCol) continue
        for (let i = 0; i < result.batch.numRows; i++) {
          const chunkId = chunkIdCol?.get(i)
          if (chunkId !== null && chunkId !== undefined && chunkId !== expectedChunkId) {
            throw new Error(`Chunk ${chunkId} received, expected ${expectedChunkId}. Chunks may be out of order or missing!`)
          }
          expectedChunkId++
          const value = chunkDataCol.get(i)
          if (value) {
            const bytes = value instanceof Uint8Array ? value : new Uint8Array(value)
            chunks.push(bytes)
            totalBytes += bytes.length
            if (onProgress) {
              const mb = (totalBytes / (1024 * 1024)).toFixed(1)
              onProgress(`Downloading trace... ${mb} MB received`)
            }
          }
        }
        break
      }
      case 'error':
        throw new Error(result.error.message)
      case 'done':
        break
    }
  }

  if (totalBytes === 0) {
    throw new Error('No trace data generated')
  }

  const combined = new Uint8Array(totalBytes)
  let offset = 0
  for (const chunk of chunks) {
    combined.set(chunk, offset)
    offset += chunk.length
  }

  return combined.buffer
}
