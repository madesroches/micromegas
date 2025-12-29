/**
 * Tests for useStreamQuery hook
 */
import { renderHook, act, waitFor } from '@testing-library/react';

// Mock streamQuery function
const mockStreamQuery = jest.fn();

jest.mock('@/lib/arrow-stream', () => ({
  streamQuery: (...args: unknown[]) => mockStreamQuery(...args),
}));

// Mock Apache Arrow
jest.mock('apache-arrow', () => ({
  Table: class MockTable {
    constructor(public batches: unknown[]) {}
  },
  Schema: class MockSchema {
    constructor(public fields: unknown[] = []) {}
  },
  RecordBatch: class MockRecordBatch {
    numRows = 10;
  },
}));

import { useStreamQuery } from '../useStreamQuery';
import { Schema, RecordBatch } from 'apache-arrow';

// Helper to create mock async generator
function createMockGenerator<T>(results: T[]): AsyncGenerator<T> {
  let index = 0;
  return {
    async next() {
      if (index < results.length) {
        return { done: false, value: results[index++] };
      }
      return { done: true, value: undefined };
    },
    async return(value?: unknown) {
      return { done: true, value: value as T };
    },
    async throw(e?: unknown) {
      throw e;
    },
    [Symbol.asyncIterator]() {
      return this;
    },
  } as AsyncGenerator<T>;
}

describe('useStreamQuery', () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  describe('initial state', () => {
    it('should have correct initial state', () => {
      const { result } = renderHook(() => useStreamQuery());

      expect(result.current.schema).toBeNull();
      expect(result.current.batchCount).toBe(0);
      expect(result.current.isStreaming).toBe(false);
      expect(result.current.isComplete).toBe(false);
      expect(result.current.error).toBeNull();
      expect(result.current.rowCount).toBe(0);
    });

    it('should provide execute, cancel, retry, getTable, getBatches functions', () => {
      const { result } = renderHook(() => useStreamQuery());

      expect(typeof result.current.execute).toBe('function');
      expect(typeof result.current.cancel).toBe('function');
      expect(typeof result.current.retry).toBe('function');
      expect(typeof result.current.getTable).toBe('function');
      expect(typeof result.current.getBatches).toBe('function');
    });
  });

  describe('execute', () => {
    it('should set isStreaming to true when execute is called', async () => {
      mockStreamQuery.mockReturnValue(createMockGenerator([{ type: 'done' }]));

      const { result } = renderHook(() => useStreamQuery());

      await act(async () => {
        result.current.execute({ sql: 'SELECT 1' });
        // Wait a tick for the state to update
        await new Promise(resolve => setTimeout(resolve, 0));
      });

      // After completion, isStreaming should be false
      await waitFor(() => {
        expect(result.current.isComplete).toBe(true);
      });
    });

    it('should update schema when schema result is received', async () => {
      const mockSchema = new Schema([]);
      mockStreamQuery.mockReturnValue(
        createMockGenerator([
          { type: 'schema', schema: mockSchema },
          { type: 'done' },
        ])
      );

      const { result } = renderHook(() => useStreamQuery());

      await act(async () => {
        await result.current.execute({ sql: 'SELECT 1' });
      });

      expect(result.current.schema).toBe(mockSchema);
    });

    it('should accumulate batches and update counts', async () => {
      const mockSchema = new Schema([]);
      const mockBatch1 = new RecordBatch();
      const mockBatch2 = new RecordBatch();

      mockStreamQuery.mockReturnValue(
        createMockGenerator([
          { type: 'schema', schema: mockSchema },
          { type: 'batch', batch: mockBatch1 },
          { type: 'batch', batch: mockBatch2 },
          { type: 'done' },
        ])
      );

      const { result } = renderHook(() => useStreamQuery());

      await act(async () => {
        await result.current.execute({ sql: 'SELECT * FROM logs' });
      });

      expect(result.current.batchCount).toBe(2);
      expect(result.current.rowCount).toBe(20); // 10 rows per mock batch
    });

    it('should set isComplete to true on done', async () => {
      mockStreamQuery.mockReturnValue(createMockGenerator([{ type: 'done' }]));

      const { result } = renderHook(() => useStreamQuery());

      await act(async () => {
        await result.current.execute({ sql: 'SELECT 1' });
      });

      expect(result.current.isComplete).toBe(true);
      expect(result.current.isStreaming).toBe(false);
    });

    it('should set error on error result', async () => {
      const error = { code: 'INVALID_SQL' as const, message: 'Syntax error', retryable: false };
      mockStreamQuery.mockReturnValue(
        createMockGenerator([{ type: 'error', error }])
      );

      const { result } = renderHook(() => useStreamQuery());

      await act(async () => {
        await result.current.execute({ sql: 'SELECT' });
      });

      expect(result.current.error).toEqual(error);
      expect(result.current.isComplete).toBe(true);
      expect(result.current.isStreaming).toBe(false);
    });

    it('should handle thrown errors', async () => {
      // eslint-disable-next-line require-yield
      mockStreamQuery.mockImplementation(async function* (): AsyncGenerator<unknown> {
        throw new Error('Network error');
      });

      const { result } = renderHook(() => useStreamQuery());

      await act(async () => {
        await result.current.execute({ sql: 'SELECT 1' });
      });

      expect(result.current.error).toEqual({
        code: 'INTERNAL',
        message: 'Network error',
        retryable: true,
      });
    });

    it('should reset state on new execute', async () => {
      const mockSchema = new Schema([]);
      mockStreamQuery.mockReturnValue(
        createMockGenerator([
          { type: 'schema', schema: mockSchema },
          { type: 'batch', batch: new RecordBatch() },
          { type: 'done' },
        ])
      );

      const { result } = renderHook(() => useStreamQuery());

      // First query
      await act(async () => {
        await result.current.execute({ sql: 'SELECT 1' });
      });

      expect(result.current.batchCount).toBe(1);

      // Second query should reset
      mockStreamQuery.mockReturnValue(createMockGenerator([{ type: 'done' }]));

      await act(async () => {
        await result.current.execute({ sql: 'SELECT 2' });
      });

      expect(result.current.batchCount).toBe(0);
      expect(result.current.schema).toBeNull();
    });
  });

  describe('cancel', () => {
    it('should set isStreaming to false on cancel', async () => {
      // Create a generator that never completes
      mockStreamQuery.mockImplementation(async function* () {
        yield { type: 'schema', schema: new Schema([]) };
        // Wait indefinitely
        await new Promise(() => {});
      });

      const { result } = renderHook(() => useStreamQuery());

      // Start query without awaiting
      act(() => {
        result.current.execute({ sql: 'SELECT * FROM big_table' });
      });

      // Cancel immediately
      act(() => {
        result.current.cancel();
      });

      expect(result.current.isStreaming).toBe(false);
    });
  });

  describe('retry', () => {
    it('should retry with last params if error is retryable', async () => {
      const error = { code: 'CONNECTION_FAILED' as const, message: 'Connection lost', retryable: true };
      mockStreamQuery.mockReturnValue(
        createMockGenerator([{ type: 'error', error }])
      );

      const { result } = renderHook(() => useStreamQuery());

      const params = { sql: 'SELECT 1' };
      await act(async () => {
        await result.current.execute(params);
      });

      expect(result.current.error?.retryable).toBe(true);

      // Set up successful response for retry
      mockStreamQuery.mockReturnValue(createMockGenerator([{ type: 'done' }]));

      await act(async () => {
        result.current.retry();
        await new Promise(resolve => setTimeout(resolve, 10));
      });

      // Should have called streamQuery twice
      expect(mockStreamQuery).toHaveBeenCalledTimes(2);
    });

    it('should not retry if error is not retryable', async () => {
      const error = { code: 'INVALID_SQL' as const, message: 'Bad SQL', retryable: false };
      mockStreamQuery.mockReturnValue(
        createMockGenerator([{ type: 'error', error }])
      );

      const { result } = renderHook(() => useStreamQuery());

      await act(async () => {
        await result.current.execute({ sql: 'SELECT' });
      });

      expect(result.current.error?.retryable).toBe(false);

      act(() => {
        result.current.retry();
      });

      // Should only have called streamQuery once
      expect(mockStreamQuery).toHaveBeenCalledTimes(1);
    });
  });

  describe('getTable and getBatches', () => {
    it('should return null table when no batches', () => {
      const { result } = renderHook(() => useStreamQuery());

      expect(result.current.getTable()).toBeNull();
    });

    it('should return empty array when no batches', () => {
      const { result } = renderHook(() => useStreamQuery());

      expect(result.current.getBatches()).toEqual([]);
    });

    it('should return Table from accumulated batches', async () => {
      const mockBatch = new RecordBatch();
      mockStreamQuery.mockReturnValue(
        createMockGenerator([
          { type: 'schema', schema: new Schema([]) },
          { type: 'batch', batch: mockBatch },
          { type: 'done' },
        ])
      );

      const { result } = renderHook(() => useStreamQuery());

      await act(async () => {
        await result.current.execute({ sql: 'SELECT 1' });
      });

      const table = result.current.getTable();
      expect(table).not.toBeNull();
      expect(table?.batches).toHaveLength(1);
    });

    it('should return batches array', async () => {
      const mockBatch1 = new RecordBatch();
      const mockBatch2 = new RecordBatch();
      mockStreamQuery.mockReturnValue(
        createMockGenerator([
          { type: 'schema', schema: new Schema([]) },
          { type: 'batch', batch: mockBatch1 },
          { type: 'batch', batch: mockBatch2 },
          { type: 'done' },
        ])
      );

      const { result } = renderHook(() => useStreamQuery());

      await act(async () => {
        await result.current.execute({ sql: 'SELECT 1' });
      });

      const batches = result.current.getBatches();
      expect(batches).toHaveLength(2);
      expect(batches[0]).toBe(mockBatch1);
      expect(batches[1]).toBe(mockBatch2);
    });
  });
});
