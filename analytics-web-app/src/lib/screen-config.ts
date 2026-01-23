/**
 * Shared configuration types for built-in pages.
 *
 * These types define the config objects that serve as the source of truth
 * for view state on built-in pages. User-defined screens use the existing
 * ScreenConfig type from screens-api.ts instead.
 *
 * Design principle: Config is source of truth; URL is a projection for sharing.
 */

/**
 * Base config shared by all built-in pages.
 * Contains fields common to all pages.
 */
export interface BaseScreenConfig {
  timeRangeFrom?: string
  timeRangeTo?: string
}

/**
 * Performance analysis page config.
 * Includes metrics selection, properties, and scale mode.
 */
export interface PerformanceAnalysisConfig extends BaseScreenConfig {
  processId: string
  selectedMeasure?: string
  selectedProperties?: string[]
  scaleMode?: 'p99' | 'max'
}

/**
 * Process metrics page config.
 * Similar to performance but without scale mode.
 */
export interface ProcessMetricsConfig extends BaseScreenConfig {
  processId: string
  selectedMeasure?: string
  selectedProperties?: string[]
}

/**
 * Process log page config.
 * Includes log filtering options.
 */
export interface ProcessLogConfig extends BaseScreenConfig {
  processId: string
  logLevel?: string
  logLimit?: number
  search?: string
}

/**
 * Processes list page config.
 * Includes search and sort options.
 */
export interface ProcessesConfig extends BaseScreenConfig {
  search?: string
  sortField?: 'exe' | 'start_time' | 'last_update_time' | 'runtime' | 'username' | 'computer'
  sortDirection?: 'asc' | 'desc'
}

/**
 * Single process detail page config.
 * Process ID is required; time range is optional.
 */
export interface ProcessPageConfig extends BaseScreenConfig {
  processId?: string
}

/**
 * User-defined screen page config.
 * Type is only used for new screens (when name is not in route).
 * Variables are synced to URL for sharing and browser history.
 */
export interface ScreenPageConfig extends BaseScreenConfig {
  type?: string
  /** Variable values from URL (variable name -> value) */
  variables?: Record<string, string>
}
