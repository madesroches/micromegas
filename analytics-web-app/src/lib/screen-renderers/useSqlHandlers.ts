import { useCallback } from 'react'
import { ScreenConfig } from '@/lib/screens-api'

/** Config interface with sql property for SQL-based renderers */
interface SqlConfig {
  sql: string
  [key: string]: unknown
}

interface SqlHandlersParams {
  /** Current working config */
  config: SqlConfig
  /** Saved config from database (null if new screen) */
  savedConfig: SqlConfig | null
  /** Callback to update config */
  onConfigChange: (config: ScreenConfig) => void
  /** Execute query function from useScreenQuery */
  execute: (sql: string) => void
}

interface SqlHandlers {
  /** Run query with new SQL, update config */
  handleRunQuery: (sql: string) => void
  /** Reset to saved SQL (or initial if new screen) and re-run */
  handleResetQuery: () => void
  /** Update config when SQL changes in editor */
  handleSqlChange: (sql: string) => void
}

/**
 * Hook providing common SQL editor handlers for screen renderers.
 *
 * Handles:
 * - Running query with new SQL
 * - Resetting to saved SQL
 * - Updating config on SQL change
 *
 * Used by MetricsRenderer and ProcessListRenderer.
 * LogRenderer has custom logic due to filter state integration.
 */
export function useSqlHandlers({
  config,
  savedConfig,
  onConfigChange,
  execute,
}: SqlHandlersParams): SqlHandlers {
  const handleRunQuery = useCallback(
    (sql: string) => {
      onConfigChange({ ...config, sql })
      execute(sql)
    },
    [config, onConfigChange, execute]
  )

  const handleResetQuery = useCallback(() => {
    const sql = savedConfig?.sql ?? config.sql
    handleRunQuery(sql as string)
  }, [savedConfig, config.sql, handleRunQuery])

  const handleSqlChange = useCallback(
    (sql: string) => {
      // Update config immediately so Save will save the current editor content
      onConfigChange({ ...config, sql })
    },
    [config, onConfigChange]
  )

  return { handleRunQuery, handleResetQuery, handleSqlChange }
}
