/**
 * Arrow IPC streaming client for query endpoint
 *
 * Implements a JSON-framed protocol parser that streams Arrow IPC data:
 * - {"type":"schema","size":N}\n followed by N bytes of schema IPC
 * - {"type":"batch","size":N}\n followed by N bytes of batch IPC
 * - {"type":"done"}\n on success
 * - {"type":"error","code":"..","message":".."}\n on error
 */

import { RecordBatch, RecordBatchReader, Schema, tableFromIPC } from 'apache-arrow';
import { getConfig } from './config';
import { authenticatedFetch, AuthenticationError } from './api';

export type ErrorCode = 'INVALID_SQL' | 'CONNECTION_FAILED' | 'INTERNAL' | 'FORBIDDEN';

interface DataHeader {
  type: 'schema' | 'batch';
  size: number;
}

interface DoneFrame {
  type: 'done';
}

interface ErrorFrame {
  type: 'error';
  code: ErrorCode;
  message: string;
}

type Frame = DataHeader | DoneFrame | ErrorFrame;

export interface StreamError {
  code: ErrorCode;
  message: string;
  retryable: boolean;
}

export type StreamResult =
  | { type: 'schema'; schema: Schema }
  | { type: 'batch'; batch: RecordBatch }
  | { type: 'done' }
  | { type: 'error'; error: StreamError };

function isRetryable(code: ErrorCode): boolean {
  return code === 'CONNECTION_FAILED';
}

/**
 * Buffered reader for processing streaming responses.
 * Handles chunk boundaries transparently for both line and binary reads.
 */
class BufferedReader {
  private chunks: Uint8Array[] = [];
  private offset = 0;
  private decoder = new TextDecoder();

  constructor(private reader: ReadableStreamDefaultReader<Uint8Array>) {}

  /**
   * Reads a newline-terminated line. Returns null if stream ends.
   */
  async readLine(): Promise<string | null> {
    let line = '';

    while (true) {
      if (this.chunks.length > 0) {
        const chunk = this.chunks[0];
        const newlineIdx = chunk.indexOf(10, this.offset); // 10 = '\n'

        if (newlineIdx !== -1) {
          line += this.decoder.decode(chunk.slice(this.offset, newlineIdx));
          this.offset = newlineIdx + 1;
          this.consumeIfExhausted();
          return line;
        }

        line += this.decoder.decode(chunk.slice(this.offset));
        this.chunks.shift();
        this.offset = 0;
      }

      const { done, value } = await this.reader.read();
      if (done) {
        return line.length > 0 ? line : null;
      }
      this.chunks.push(value);
    }
  }

  /**
   * Reads exactly `size` bytes.
   */
  async readBytes(size: number): Promise<Uint8Array> {
    const result = new Uint8Array(size);
    let written = 0;

    while (written < size) {
      if (this.chunks.length > 0) {
        const chunk = this.chunks[0];
        const available = chunk.length - this.offset;
        const needed = size - written;
        const toCopy = Math.min(available, needed);

        result.set(chunk.slice(this.offset, this.offset + toCopy), written);
        written += toCopy;
        this.offset += toCopy;
        this.consumeIfExhausted();
        continue;
      }

      const { done, value } = await this.reader.read();
      if (done) {
        throw new Error(`Unexpected end of stream, expected ${size - written} more bytes`);
      }
      this.chunks.push(value);
    }

    return result;
  }

  private consumeIfExhausted(): void {
    if (this.chunks.length > 0 && this.offset >= this.chunks[0].length) {
      this.chunks.shift();
      this.offset = 0;
    }
  }

  release(): void {
    this.reader.releaseLock();
  }
}

export interface StreamQueryParams {
  sql: string;
  params?: Record<string, string>;
  begin?: string; // ISO date
  end?: string;   // ISO date
}

/**
 * Streams query results as Arrow RecordBatches.
 *
 * Parses the JSON-framed protocol from the backend and yields:
 * - schema: The Arrow schema for the result set
 * - batch: Individual RecordBatches as they arrive
 * - done: Query completed successfully
 * - error: An error occurred (may have partial results)
 */
export async function* streamQuery(
  params: StreamQueryParams,
  signal?: AbortSignal
): AsyncGenerator<StreamResult> {
  const basePath = getConfig().basePath;

  const response = await authenticatedFetch(`${basePath}/query-stream`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      sql: params.sql,
      params: params.params || {},
      begin: params.begin,
      end: params.end,
    }),
    signal,
  });

  if (!response.ok) {
    if (response.status === 401) {
      throw new AuthenticationError();
    }
    if (response.status === 403) {
      // Forbidden error is returned as a stream with error frame, but also as HTTP 403
      // Parse the response body for the error message
      const text = await response.text();
      try {
        const frame = JSON.parse(text.trim()) as ErrorFrame;
        yield {
          type: 'error',
          error: {
            code: frame.code,
            message: frame.message,
            retryable: isRetryable(frame.code),
          },
        };
        return;
      } catch {
        throw new Error(`HTTP 403: ${text}`);
      }
    }
    const text = await response.text();
    throw new Error(`HTTP ${response.status}: ${text}`);
  }

  if (!response.body) {
    throw new Error('No response body');
  }

  const bufferedReader = new BufferedReader(response.body.getReader());

  // Collect all IPC bytes for parsing
  const ipcChunks: Uint8Array[] = [];
  let capturedError: StreamError | null = null;
  let schemaYielded = false;

  try {
    while (true) {
      const line = await bufferedReader.readLine();
      if (line === null) {
        break;
      }

      let frame: Frame;
      try {
        frame = JSON.parse(line);
      } catch {
        capturedError = {
          code: 'INTERNAL',
          message: `Invalid frame: ${line.slice(0, 100)}`,
          retryable: false,
        };
        break;
      }

      switch (frame.type) {
        case 'schema':
        case 'batch': {
          const bytes = await bufferedReader.readBytes(frame.size);
          ipcChunks.push(bytes);

          // Try to parse what we have so far
          if (frame.type === 'schema') {
            // For schema, we can yield it immediately by parsing the IPC data
            try {
              const table = tableFromIPC(bytes);
              yield { type: 'schema', schema: table.schema };
              schemaYielded = true;
            } catch (e) {
              capturedError = {
                code: 'INTERNAL',
                message: `Failed to parse schema: ${e}`,
                retryable: false,
              };
            }
          } else if (frame.type === 'batch' && schemaYielded) {
            // For batches, we need to combine with schema to parse
            // Accumulate all IPC data and parse incrementally
            try {
              const combinedBuffer = concatenateBuffers(ipcChunks);
              const reader = await RecordBatchReader.from(combinedBuffer);

              // Yield the latest batch (the reader will have parsed all batches)
              let lastBatch: RecordBatch | undefined;
              for await (const batch of reader) {
                lastBatch = batch;
              }
              if (lastBatch) {
                yield { type: 'batch', batch: lastBatch };
              }
            } catch (e) {
              // If parsing fails, it might be due to incomplete data
              // Continue accumulating and try again with the next batch
              console.warn('Batch parsing warning:', e);
            }
          }
          break;
        }
        case 'done': {
          yield { type: 'done' };
          return;
        }
        case 'error': {
          capturedError = {
            code: frame.code,
            message: frame.message,
            retryable: isRetryable(frame.code),
          };
          break;
        }
      }

      if (capturedError) {
        break;
      }
    }

    // If we captured an error, report it
    if (capturedError) {
      yield { type: 'error', error: capturedError };
    }
  } finally {
    bufferedReader.release();
  }
}

/**
 * Concatenate multiple Uint8Array buffers into one
 */
function concatenateBuffers(buffers: Uint8Array[]): Uint8Array {
  const totalLength = buffers.reduce((sum, buf) => sum + buf.length, 0);
  const result = new Uint8Array(totalLength);
  let offset = 0;
  for (const buf of buffers) {
    result.set(buf, offset);
    offset += buf.length;
  }
  return result;
}

/**
 * Simple helper to execute a streaming query and collect all results
 * Returns a Table for easier consumption
 */
export async function executeStreamQuery(
  params: StreamQueryParams,
  signal?: AbortSignal
): Promise<{ schema: Schema | null; batches: RecordBatch[]; error: StreamError | null }> {
  const batches: RecordBatch[] = [];
  let schema: Schema | null = null;
  let error: StreamError | null = null;

  for await (const result of streamQuery(params, signal)) {
    switch (result.type) {
      case 'schema':
        schema = result.schema;
        break;
      case 'batch':
        batches.push(result.batch);
        break;
      case 'error':
        error = result.error;
        break;
      case 'done':
        break;
    }
  }

  return { schema, batches, error };
}
