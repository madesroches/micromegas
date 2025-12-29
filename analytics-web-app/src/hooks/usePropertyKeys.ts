import { useState, useEffect, useRef, useCallback } from 'react'
import { executeStreamQuery } from '@/lib/arrow-stream'

const PROPERTY_KEYS_SQL = `SELECT DISTINCT unnest(arrow_cast(jsonb_object_keys(properties), 'List(Utf8)')) as key
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
  AND properties IS NOT NULL
ORDER BY key`

interface UsePropertyKeysParams {
  processId: string | null
  measureName: string | null
  apiTimeRange: { begin: string; end: string }
  enabled?: boolean
}

interface UsePropertyKeysReturn {
  keys: string[]
  isLoading: boolean
  error: string | null
  refetch: () => void
}

export function usePropertyKeys({
  processId,
  measureName,
  apiTimeRange,
  enabled = true,
}: UsePropertyKeysParams): UsePropertyKeysReturn {
  const [keys, setKeys] = useState<string[]>([])
  const [error, setError] = useState<string | null>(null)
  const [isLoading, setIsLoading] = useState(false)

  const fetchKeys = useCallback(async () => {
    if (!processId || !measureName || !enabled) return

    setIsLoading(true)
    setError(null)

    try {
      const { batches, error: streamError } = await executeStreamQuery({
        sql: PROPERTY_KEYS_SQL,
        params: { process_id: processId, measure_name: measureName },
        begin: apiTimeRange.begin,
        end: apiTimeRange.end,
      })

      if (streamError) {
        setError(streamError.message)
        setKeys([])
        setIsLoading(false)
        return
      }

      const keyList: string[] = []
      for (const batch of batches) {
        for (let i = 0; i < batch.numRows; i++) {
          const row = batch.get(i)
          if (row) {
            const key = String(row.key ?? '')
            if (key) keyList.push(key)
          }
        }
      }

      setKeys(keyList)
      setError(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unknown error')
      setKeys([])
    } finally {
      setIsLoading(false)
    }
  }, [processId, measureName, apiTimeRange.begin, apiTimeRange.end, enabled])

  const prevParamsRef = useRef<{
    processId: string | null
    measureName: string | null
    begin: string
    end: string
  } | null>(null)

  useEffect(() => {
    if (!enabled || !processId || !measureName) {
      return
    }

    const currentParams = {
      processId,
      measureName,
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    }

    // Check if params changed
    const paramsChanged =
      !prevParamsRef.current ||
      prevParamsRef.current.processId !== currentParams.processId ||
      prevParamsRef.current.measureName !== currentParams.measureName ||
      prevParamsRef.current.begin !== currentParams.begin ||
      prevParamsRef.current.end !== currentParams.end

    if (paramsChanged) {
      prevParamsRef.current = currentParams
      fetchKeys()
    }
  }, [processId, measureName, apiTimeRange.begin, apiTimeRange.end, enabled, fetchKeys])

  return {
    keys,
    isLoading,
    error,
    refetch: fetchKeys,
  }
}
