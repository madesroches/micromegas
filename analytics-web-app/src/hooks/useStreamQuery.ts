/**
 * React hook for streaming Arrow IPC query results
 *
 * Provides progressive loading with state updates as batches arrive.
 */

import { useState, useCallback, useRef } from 'react';
import { Table, Schema, RecordBatch } from 'apache-arrow';
import { streamQuery, StreamError, StreamQueryParams } from '@/lib/arrow-stream';

export interface StreamQueryState {
  schema: Schema | null;
  batchCount: number;
  isStreaming: boolean;
  isComplete: boolean;
  error: StreamError | null;
  rowCount: number;
}

export interface UseStreamQueryReturn extends StreamQueryState {
  /** Execute a streaming query */
  execute: (params: StreamQueryParams) => Promise<void>;
  /** Cancel the current query */
  cancel: () => void;
  /** Retry the last query (only if error was retryable) */
  retry: () => void;
  /** Get the accumulated Table from all batches */
  getTable: () => Table | null;
  /** Get the raw batches array */
  getBatches: () => RecordBatch[];
}

export function useStreamQuery(): UseStreamQueryReturn {
  const [state, setState] = useState<StreamQueryState>({
    schema: null,
    batchCount: 0,
    isStreaming: false,
    isComplete: false,
    error: null,
    rowCount: 0,
  });

  const abortRef = useRef<AbortController | null>(null);
  // Mutable array to avoid O(n²) allocations from spreading
  const batchesRef = useRef<RecordBatch[]>([]);
  // Store last params for retry
  const lastParamsRef = useRef<StreamQueryParams | null>(null);

  const execute = useCallback(async (params: StreamQueryParams) => {
    // Cancel any existing query
    abortRef.current?.abort();
    abortRef.current = new AbortController();
    batchesRef.current = [];
    lastParamsRef.current = params;

    setState({
      schema: null,
      batchCount: 0,
      isStreaming: true,
      isComplete: false,
      error: null,
      rowCount: 0,
    });

    try {
      for await (const result of streamQuery(params, abortRef.current.signal)) {
        switch (result.type) {
          case 'schema':
            setState(s => ({ ...s, schema: result.schema }));
            break;
          case 'batch':
            batchesRef.current.push(result.batch);
            setState(s => ({
              ...s,
              batchCount: batchesRef.current.length,
              rowCount: s.rowCount + result.batch.numRows,
            }));
            break;
          case 'done':
            setState(s => ({ ...s, isStreaming: false, isComplete: true }));
            break;
          case 'error':
            setState(s => ({
              ...s,
              error: result.error,
              isStreaming: false,
              isComplete: true,
            }));
            break;
        }
      }
    } catch (e) {
      if (e instanceof Error && e.name !== 'AbortError') {
        setState(s => ({
          ...s,
          error: { code: 'INTERNAL', message: e.message, retryable: true },
          isStreaming: false,
          isComplete: true,
        }));
      }
    }
  }, []);

  const cancel = useCallback(() => {
    abortRef.current?.abort();
    setState(s => ({ ...s, isStreaming: false }));
  }, []);

  const retry = useCallback(() => {
    if (state.error?.retryable && lastParamsRef.current) {
      execute(lastParamsRef.current);
    }
  }, [state.error, execute]);

  const getTable = useCallback((): Table | null => {
    if (batchesRef.current.length === 0) return null;
    return new Table(batchesRef.current);
  }, []);

  const getBatches = useCallback((): RecordBatch[] => {
    return batchesRef.current;
  }, []);

  return { ...state, execute, cancel, retry, getTable, getBatches };
}
