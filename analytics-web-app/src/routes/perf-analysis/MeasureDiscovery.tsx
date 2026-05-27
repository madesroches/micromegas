/**
 * Measure discovery for the Performance Analysis page.
 *
 * Encapsulates the discovery query (distinct measures for the process) and the
 * measure dropdown. The discovered list, the "done"/"loading" flags, the
 * selected measure, and the query error stay owned by the page because the
 * fetch-orchestration gate and the metrics/no-data UI all read them — this
 * component owns the fetch logic and the dropdown, and registers its loader
 * into the page-held ref so the gate triggers it exactly as before (#1089).
 */
import { useCallback, type Dispatch, type MutableRefObject, type SetStateAction } from 'react'
import { executeStreamQuery } from '@/lib/arrow-stream'
import { DISCOVERY_SQL, type Measure } from './queries'

interface MeasureDiscoveryProps {
  processId: string
  timeRange: { begin: string; end: string }
  dataSource?: string
  selectedMeasure: string | null
  measures: Measure[]
  discoveryLoading: boolean
  noMeasuresAvailable: boolean
  setMeasures: Dispatch<SetStateAction<Measure[]>>
  setDiscoveryDone: Dispatch<SetStateAction<boolean>>
  setDiscoveryLoading: Dispatch<SetStateAction<boolean>>
  onError: (message: string) => void
  onAutoSelect: (name: string) => void
  onMeasureChange: (name: string) => void
  /** Page registers this loader so the time-range gate can re-trigger it. */
  loadRef: MutableRefObject<(() => Promise<void>) | null>
}

export function MeasureDiscovery({
  processId,
  timeRange,
  dataSource,
  selectedMeasure,
  measures,
  discoveryLoading,
  noMeasuresAvailable,
  setMeasures,
  setDiscoveryDone,
  setDiscoveryLoading,
  onError,
  onAutoSelect,
  onMeasureChange,
  loadRef,
}: MeasureDiscoveryProps) {
  const loadDiscovery = useCallback(async () => {
    if (!processId) return
    setDiscoveryLoading(true)

    try {
      const { batches, error } = await executeStreamQuery({
        sql: DISCOVERY_SQL,
        params: { process_id: processId },
        begin: timeRange.begin,
        end: timeRange.end,
        dataSource,
      })

      if (error) {
        onError(error.message)
        setDiscoveryDone(true)
        setDiscoveryLoading(false)
        return
      }

      const measureList: Measure[] = []
      for (const batch of batches) {
        for (let i = 0; i < batch.numRows; i++) {
          const row = batch.get(i)
          if (row) {
            measureList.push({
              name: String(row.name ?? ''),
              target: String(row.target ?? ''),
              unit: String(row.unit ?? ''),
            })
          }
        }
      }

      setMeasures(measureList)
      setDiscoveryDone(true)

      // Auto-select measure if none specified - use DeltaTime if available, else first
      if (measureList.length > 0 && !selectedMeasure) {
        const deltaTime = measureList.find((m) => m.name === 'DeltaTime')
        const autoMeasure = deltaTime ? deltaTime.name : measureList[0].name
        onAutoSelect(autoMeasure)
      }
    } catch (err) {
      onError(err instanceof Error ? err.message : 'Unknown error')
      setDiscoveryDone(true)
    } finally {
      setDiscoveryLoading(false)
    }
  }, [
    processId,
    timeRange.begin,
    timeRange.end,
    selectedMeasure,
    dataSource,
    setMeasures,
    setDiscoveryDone,
    setDiscoveryLoading,
    onError,
    onAutoSelect,
  ])

  // Always expose the latest loader to the page without re-running effects.
  loadRef.current = loadDiscovery

  return (
    <select
      value={selectedMeasure || ''}
      onChange={(e) => onMeasureChange(e.target.value)}
      disabled={noMeasuresAvailable || (discoveryLoading && measures.length === 0)}
      className="min-w-[250px] px-3 py-2 bg-app-panel border border-theme-border rounded-md text-theme-text-primary text-sm focus:outline-none focus:border-accent-link disabled:opacity-50 disabled:cursor-not-allowed"
    >
      {measures.length > 0 ? (
        measures.map((m) => (
          <option key={m.name} value={m.name}>
            {m.name} ({m.unit})
          </option>
        ))
      ) : noMeasuresAvailable ? (
        <option value="">No measures available</option>
      ) : (
        <option value="">Loading measures...</option>
      )}
    </select>
  )
}
