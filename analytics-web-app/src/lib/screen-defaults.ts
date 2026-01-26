/**
 * Default values for screen configurations.
 *
 * Centralizes defaults to avoid duplication and ensure consistency
 * between components that need to reference these values.
 */

/** Default time range for built-in pages (processes, logs, etc.) */
export const DEFAULT_TIME_RANGE = {
  from: 'now-5m',
  to: 'now',
} as const

/** Default time range for user screens (notebooks) */
export const DEFAULT_SCREEN_TIME_RANGE = {
  from: 'now-5m',
  to: 'now',
} as const

/** Default log level filter (show all levels) */
export const DEFAULT_LOG_LEVEL = 'all'

/** Default row limit for log queries */
export const DEFAULT_LOG_LIMIT = 1000
