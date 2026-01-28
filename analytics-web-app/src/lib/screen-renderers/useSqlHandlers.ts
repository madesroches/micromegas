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
  /** Set unsaved changes state */
  setHasUnsavedChanges: (value: boolean) => void
  /** Execute query function from useScreenQuery */
  execute: (sql: string) => void
}

interface SqlHandlers {
  /** Run query with new SQL, update config and mark unsaved if changed */
  handleRunQuery: (sql: string) => void
  /** Reset to saved SQL (or initial if new screen) and re-run */
  handleResetQuery: () => void
  /** Mark unsaved when SQL changes in editor */
  handleSqlChange: (sql: string) => void
}

/**
 * Hook providing common SQL editor handlers for screen renderers.
 *
 * Handles:
 * - Running query with new SQL
 * - Resetting to saved SQL
 * - Tracking unsaved changes
 *
 * Used by MetricsRenderer and ProcessListRenderer.
 * LogRenderer has custom logic due to filter state integration.
 */
export function useSqlHandlers({
  config,
  savedConfig,
  onConfigChange,
  setHasUnsavedChanges,
  execute,
}: SqlHandlersParams): SqlHandlers {
  const handleRunQuery = useCallback(
    (sql: string) => {
      onConfigChange({ ...config, sql })
      if (savedConfig) {
        setHasUnsavedChanges(sql !== savedConfig.sql)
      }
      execute(sql)
    },
    [config, savedConfig, onConfigChange, setHasUnsavedChanges, execute]
  )

  const handleResetQuery = useCallback(() => {
    const sql = savedConfig?.sql ?? config.sql
    handleRunQuery(sql as string)
  }, [savedConfig, config.sql, handleRunQuery])

  const handleSqlChange = useCallback(
    (sql: string) => {
      // Update config immediately so Save will save the current editor content
      onConfigChange({ ...config, sql })
      if (savedConfig) {
        setHasUnsavedChanges(sql !== savedConfig.sql)
      }
    },
    [config, savedConfig, onConfigChange, setHasUnsavedChanges]
  )

  return { handleRunQuery, handleResetQuery, handleSqlChange }
}
