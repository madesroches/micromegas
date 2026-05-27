/**
 * Pure query/config helpers for the Performance Analysis page.
 *
 * Extracted from PerformanceAnalysisPage.tsx (#1089) so the SQL templates,
 * URL serialization, and bin-interval math can be unit-tested in isolation.
 * No React, no side effects.
 */
import type { PerformanceAnalysisConfig } from '@/lib/screen-config'

export const DISCOVERY_SQL = `SELECT DISTINCT name, target, unit
FROM view_instance('measures', '$process_id')
ORDER BY name`

export const DEFAULT_SQL = `SELECT
  date_bin(INTERVAL '$bin_interval', time) as time,
  max(value) as value,
  jsonb_format_json(first_value(properties) FILTER (WHERE properties IS NOT NULL)) as properties
FROM view_instance('measures', '$process_id')
WHERE name = '$measure_name'
GROUP BY date_bin(INTERVAL '$bin_interval', time)
ORDER BY time`

export const THREAD_COVERAGE_SQL = `SELECT
  arrow_cast(stream_id, 'Utf8') as stream_id,
  concat(
    arrow_cast(property_get("streams.properties", 'thread-name'), 'Utf8'),
    '-',
    arrow_cast(property_get("streams.properties", 'thread-id'), 'Utf8')
  ) as thread_name,
  begin_time,
  end_time
FROM blocks
WHERE process_id = '$process_id'
  AND array_has("streams.tags", 'cpu')
ORDER BY stream_id, begin_time`

export const TRACE_EVENTS_COUNT_SQL = `SELECT
  SUM(nb_objects) as event_count
FROM blocks
WHERE process_id = '$process_id'
  AND array_has("streams.tags", 'cpu')`

export const VARIABLES = [
  { name: 'process_id', description: 'Current process ID' },
  { name: 'measure_name', description: 'Selected measure name' },
  { name: 'bin_interval', description: 'Time bucket size for downsampling' },
]

// Default config for PerformanceAnalysisPage
export const DEFAULT_CONFIG: PerformanceAnalysisConfig = {
  processId: '',
  timeRangeFrom: 'now-1h',
  timeRangeTo: 'now',
  selectedMeasure: undefined,
  selectedProperties: [],
  scaleMode: 'p99',
}

export interface Measure {
  name: string
  target: string
  unit: string
}

// URL builder for PerformanceAnalysisPage - builds query string from config
export const buildUrl = (cfg: PerformanceAnalysisConfig): string => {
  const params = new URLSearchParams()
  if (cfg.processId) params.set('process_id', cfg.processId)
  if (cfg.timeRangeFrom && cfg.timeRangeFrom !== DEFAULT_CONFIG.timeRangeFrom) {
    params.set('from', cfg.timeRangeFrom)
  }
  if (cfg.timeRangeTo && cfg.timeRangeTo !== DEFAULT_CONFIG.timeRangeTo) {
    params.set('to', cfg.timeRangeTo)
  }
  if (cfg.selectedMeasure) params.set('measure', cfg.selectedMeasure)
  if (cfg.selectedProperties && cfg.selectedProperties.length > 0) {
    params.set('properties', cfg.selectedProperties.join(','))
  }
  if (cfg.scaleMode && cfg.scaleMode !== 'p99') params.set('scale', cfg.scaleMode)
  const qs = params.toString()
  return qs ? `?${qs}` : ''
}

export function calculateBinInterval(timeSpanMs: number, chartWidthPx: number = 800): string {
  const numBins = chartWidthPx
  const binIntervalMs = timeSpanMs / numBins

  const intervals = [
    { ms: 1, label: '1 millisecond' },
    { ms: 10, label: '10 milliseconds' },
    { ms: 50, label: '50 milliseconds' },
    { ms: 100, label: '100 milliseconds' },
    { ms: 500, label: '500 milliseconds' },
    { ms: 1000, label: '1 second' },
    { ms: 5000, label: '5 seconds' },
    { ms: 10000, label: '10 seconds' },
    { ms: 30000, label: '30 seconds' },
    { ms: 60000, label: '1 minute' },
    { ms: 300000, label: '5 minutes' },
    { ms: 600000, label: '10 minutes' },
    { ms: 1800000, label: '30 minutes' },
    { ms: 3600000, label: '1 hour' },
  ]

  for (const interval of intervals) {
    if (interval.ms >= binIntervalMs) {
      return interval.label
    }
  }
  return '1 hour'
}
