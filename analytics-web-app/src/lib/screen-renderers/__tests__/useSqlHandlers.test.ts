/**
 * Tests for useSqlHandlers hook
 */
import { renderHook, act } from '@testing-library/react'
import { useSqlHandlers } from '../useSqlHandlers'

describe('useSqlHandlers', () => {
  const createMocks = () => ({
    onConfigChange: jest.fn(),
    setHasUnsavedChanges: jest.fn(),
    execute: jest.fn(),
  })

  describe('handleSqlChange', () => {
    it('should update config when SQL changes (regression test for save bug)', () => {
      // This test ensures that editing SQL updates the config immediately,
      // so that clicking Save will persist the current editor content.
      // Previously, handleSqlChange only called setHasUnsavedChanges without
      // updating the config, causing Save to save stale SQL.
      const mocks = createMocks()
      const config = { sql: 'SELECT * FROM old_query' }
      const savedConfig = { sql: 'SELECT * FROM old_query' }

      const { result } = renderHook(() =>
        useSqlHandlers({
          config,
          savedConfig,
          ...mocks,
        })
      )

      act(() => {
        result.current.handleSqlChange('SELECT * FROM new_query')
      })

      // Config should be updated with new SQL
      expect(mocks.onConfigChange).toHaveBeenCalledWith({
        sql: 'SELECT * FROM new_query',
      })
      // Should also mark as unsaved
      expect(mocks.setHasUnsavedChanges).toHaveBeenCalledWith(true)
    })

    it('should mark as not unsaved when SQL matches saved config', () => {
      const mocks = createMocks()
      const config = { sql: 'SELECT * FROM modified' }
      const savedConfig = { sql: 'SELECT * FROM original' }

      const { result } = renderHook(() =>
        useSqlHandlers({
          config,
          savedConfig,
          ...mocks,
        })
      )

      act(() => {
        result.current.handleSqlChange('SELECT * FROM original')
      })

      expect(mocks.setHasUnsavedChanges).toHaveBeenCalledWith(false)
    })

    it('should still update config when savedConfig is null (new screen)', () => {
      const mocks = createMocks()
      const config = { sql: 'SELECT 1' }

      const { result } = renderHook(() =>
        useSqlHandlers({
          config,
          savedConfig: null,
          ...mocks,
        })
      )

      act(() => {
        result.current.handleSqlChange('SELECT 2')
      })

      // Config should still be updated even without savedConfig
      expect(mocks.onConfigChange).toHaveBeenCalledWith({ sql: 'SELECT 2' })
      // setHasUnsavedChanges should not be called when savedConfig is null
      expect(mocks.setHasUnsavedChanges).not.toHaveBeenCalled()
    })
  })

  describe('handleRunQuery', () => {
    it('should update config, mark unsaved, and execute query', () => {
      const mocks = createMocks()
      const config = { sql: 'SELECT * FROM old' }
      const savedConfig = { sql: 'SELECT * FROM old' }

      const { result } = renderHook(() =>
        useSqlHandlers({
          config,
          savedConfig,
          ...mocks,
        })
      )

      act(() => {
        result.current.handleRunQuery('SELECT * FROM new')
      })

      expect(mocks.onConfigChange).toHaveBeenCalledWith({ sql: 'SELECT * FROM new' })
      expect(mocks.setHasUnsavedChanges).toHaveBeenCalledWith(true)
      expect(mocks.execute).toHaveBeenCalledWith('SELECT * FROM new')
    })

    it('should not mark unsaved when SQL matches saved config', () => {
      const mocks = createMocks()
      const config = { sql: 'SELECT * FROM table' }
      const savedConfig = { sql: 'SELECT * FROM table' }

      const { result } = renderHook(() =>
        useSqlHandlers({
          config,
          savedConfig,
          ...mocks,
        })
      )

      act(() => {
        result.current.handleRunQuery('SELECT * FROM table')
      })

      expect(mocks.setHasUnsavedChanges).toHaveBeenCalledWith(false)
    })
  })

  describe('handleResetQuery', () => {
    it('should reset to saved SQL and execute', () => {
      const mocks = createMocks()
      const config = { sql: 'SELECT * FROM modified' }
      const savedConfig = { sql: 'SELECT * FROM original' }

      const { result } = renderHook(() =>
        useSqlHandlers({
          config,
          savedConfig,
          ...mocks,
        })
      )

      act(() => {
        result.current.handleResetQuery()
      })

      expect(mocks.onConfigChange).toHaveBeenCalledWith({ sql: 'SELECT * FROM original' })
      expect(mocks.execute).toHaveBeenCalledWith('SELECT * FROM original')
    })

    it('should use current config SQL when savedConfig is null', () => {
      const mocks = createMocks()
      const config = { sql: 'SELECT * FROM current' }

      const { result } = renderHook(() =>
        useSqlHandlers({
          config,
          savedConfig: null,
          ...mocks,
        })
      )

      act(() => {
        result.current.handleResetQuery()
      })

      expect(mocks.execute).toHaveBeenCalledWith('SELECT * FROM current')
    })
  })
})
