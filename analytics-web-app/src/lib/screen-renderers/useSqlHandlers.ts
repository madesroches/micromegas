import { useCallback } from 'react'
import { ScreenConfig } from '@/lib/screens-api'

interface SqlHandlersParams<T extends ScreenConfig> {
  /** Current working config */
  config: T
  /** Saved config from database (null if new screen) */
  savedConfig: T | null
  /** Callback to update config */
  onConfigChange: (config: T) => void
  /** Callback when there are unsaved changes */
  onUnsavedChange: () => void
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
export function useSqlHandlers<T extends ScreenConfig>({
  config,
  savedConfig,
  onConfigChange,
  onUnsavedChange,
  execute,
}: SqlHandlersParams<T>): SqlHandlers {
  const handleRunQuery = useCallback(
    (sql: string) => {
      onConfigChange({ ...config, sql } as T)
      if (savedConfig && sql !== savedConfig.sql) {
        onUnsavedChange()
      }
      execute(sql)
    },
    [config, savedConfig, onConfigChange, onUnsavedChange, execute]
  )

  const handleResetQuery = useCallback(() => {
    const sql = savedConfig?.sql ?? config.sql
    handleRunQuery(sql)
  }, [savedConfig, config.sql, handleRunQuery])

  const handleSqlChange = useCallback(
    (sql: string) => {
      if (savedConfig && sql !== savedConfig.sql) {
        onUnsavedChange()
      }
    },
    [savedConfig, onUnsavedChange]
  )

  return { handleRunQuery, handleResetQuery, handleSqlChange }
}
