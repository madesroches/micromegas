/**
 * URL parameter parsing utilities.
 *
 * Handles parsing URL search params into config objects.
 * Used by useScreenConfig hook to initialize state from URL.
 *
 * Conventions:
 * - Arrays are comma-separated: ?properties=cpu,memory,disk
 * - Empty arrays omit the param entirely
 * - Default values are omitted to keep URLs clean
 *
 * Note: Each page has its own buildUrl function that serializes config to URL.
 * This keeps URL structure close to the page that owns it.
 */

import type { BaseScreenConfig } from './screen-config'

/**
 * Parse URL search params into a partial config object.
 * Only includes fields that are present in the URL.
 *
 * @param params - URLSearchParams to parse
 * @returns Partial config with values from URL
 */
export function parseUrlParams(params: URLSearchParams): Partial<BaseScreenConfig> & Record<string, unknown> {
  const result: Record<string, unknown> = {}

  // String params
  // Support both 'id' and 'process_id' for processId (ProcessPage uses 'id')
  if (params.has('id')) result.processId = params.get('id')
  if (params.has('process_id')) result.processId = params.get('process_id')
  if (params.has('type')) result.type = params.get('type')
  if (params.has('from')) result.timeRangeFrom = params.get('from')
  if (params.has('to')) result.timeRangeTo = params.get('to')
  if (params.has('measure')) result.selectedMeasure = params.get('measure')
  if (params.has('scale')) result.scaleMode = params.get('scale')
  if (params.has('level')) result.logLevel = params.get('level')
  if (params.has('search')) result.search = params.get('search')
  if (params.has('sort')) result.sortField = params.get('sort')
  if (params.has('dir')) result.sortDirection = params.get('dir')

  // Number params
  if (params.has('limit')) {
    const limitStr = params.get('limit')
    if (limitStr) {
      const parsed = parseInt(limitStr, 10)
      if (!isNaN(parsed)) result.logLimit = parsed
    }
  }

  // Array params (comma-separated)
  if (params.has('properties')) {
    const val = params.get('properties')
    result.selectedProperties = val ? val.split(',').filter(Boolean) : []
  }

  return result
}
