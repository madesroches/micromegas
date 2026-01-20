/**
 * URL parameter parsing and building utilities.
 *
 * These utilities handle the bidirectional mapping between config objects
 * and URL search params. Used by useScreenConfig hook and page-specific
 * URL builders.
 *
 * Conventions:
 * - Arrays are comma-separated: ?properties=cpu,memory,disk
 * - Empty arrays omit the param entirely
 * - Default values are omitted to keep URLs clean
 */

import type { BaseScreenConfig } from './screen-config'

/**
 * Parameter name mapping from config fields to URL param names.
 * Config uses camelCase, URLs use snake_case for readability.
 */
export const PARAM_MAP = {
  processId: 'process_id',
  timeRangeFrom: 'from',
  timeRangeTo: 'to',
  selectedMeasure: 'measure',
  selectedProperties: 'properties',
  scaleMode: 'scale',
  logLevel: 'level',
  logLimit: 'limit',
  search: 'search',
  sortField: 'sort',
  sortDirection: 'dir',
} as const

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
  if (params.has('process_id')) result.processId = params.get('process_id')
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

/**
 * Build URLSearchParams from a partial config object.
 * Only includes non-empty values.
 *
 * @param config - Partial config to serialize
 * @returns URLSearchParams with config values
 */
export function buildUrlParams(config: Record<string, unknown>): URLSearchParams {
  const params = new URLSearchParams()

  // String params
  if (config.processId) params.set('process_id', String(config.processId))
  if (config.timeRangeFrom) params.set('from', String(config.timeRangeFrom))
  if (config.timeRangeTo) params.set('to', String(config.timeRangeTo))
  if (config.selectedMeasure) params.set('measure', String(config.selectedMeasure))
  if (config.scaleMode) params.set('scale', String(config.scaleMode))
  if (config.logLevel && config.logLevel !== 'all') params.set('level', String(config.logLevel))
  if (config.search) params.set('search', String(config.search))
  if (config.sortField) params.set('sort', String(config.sortField))
  if (config.sortDirection) params.set('dir', String(config.sortDirection))

  // Number params
  if (config.logLimit && config.logLimit !== 100) params.set('limit', String(config.logLimit))

  // Array params (comma-separated)
  const properties = config.selectedProperties as string[] | undefined
  if (properties && properties.length > 0) {
    params.set('properties', properties.join(','))
  }

  return params
}
