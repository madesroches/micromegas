import { useState, useEffect, useRef, useCallback } from 'react'
import { useMutation } from '@tanstack/react-query'
import { executeSqlQuery, toRowObjects } from '@/lib/api'

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

  const mutation = useMutation({
    mutationFn: executeSqlQuery,
    onSuccess: (data) => {
      const rows = toRowObjects(data)
      const keyList = rows.map((row) => String(row.key ?? '')).filter(Boolean)
      setKeys(keyList)
      setError(null)
    },
    onError: (err: Error) => {
      setError(err.message)
      setKeys([])
    },
  })

  const mutateRef = useRef(mutation.mutate)
  mutateRef.current = mutation.mutate

  const fetchKeys = useCallback(() => {
    if (!processId || !measureName || !enabled) return
    mutateRef.current({
      sql: PROPERTY_KEYS_SQL,
      params: { process_id: processId, measure_name: measureName },
      begin: apiTimeRange.begin,
      end: apiTimeRange.end,
    })
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
    isLoading: mutation.isPending,
    error,
    refetch: fetchKeys,
  }
}
