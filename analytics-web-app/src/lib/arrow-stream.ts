/**
 * Arrow IPC streaming client for query endpoint
 *
 * Implements a JSON-framed protocol parser that streams Arrow IPC data:
 * - {"type":"schema","size":N}\n followed by N bytes of schema IPC
 * - {"type":"batch","size":N}\n followed by N bytes of batch IPC
 * - {"type":"done"}\n on success
 * - {"type":"error","code":"..","message":".."}\n on error
 */

import { RecordBatch, RecordBatchReader, Schema } from 'apache-arrow';
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

    // eslint-disable-next-line no-constant-condition
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

  // Keep schema bytes for parsing batches (each batch needs schema context)
  let schemaBytes: Uint8Array | null = null;
  let capturedError: StreamError | null = null;

  // Reusable buffer for combining schema + batch bytes, grows as needed
  // Local to this generator to avoid interference between concurrent queries
  let parseBuffer: Uint8Array | null = null;

  const combineForParsing = (schema: Uint8Array, batch: Uint8Array): Uint8Array => {
    const requiredSize = schema.length + batch.length;
    if (!parseBuffer || parseBuffer.length < requiredSize) {
      const newSize = Math.max(requiredSize, parseBuffer?.length ? parseBuffer.length * 2 : 64 * 1024);
      parseBuffer = new Uint8Array(newSize);
    }
    parseBuffer.set(schema, 0);
    parseBuffer.set(batch, schema.length);
    return parseBuffer.subarray(0, requiredSize);
  };

  try {
    // eslint-disable-next-line no-constant-condition
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
        case 'schema': {
          const bytes = await bufferedReader.readBytes(frame.size);
          schemaBytes = bytes;
          try {
            const reader = await RecordBatchReader.from(bytes);
            yield { type: 'schema', schema: reader.schema };
          } catch (e) {
            capturedError = {
              code: 'INTERNAL',
              message: `Failed to parse schema: ${e}`,
              retryable: false,
            };
          }
          break;
        }
        case 'batch': {
          const bytes = await bufferedReader.readBytes(frame.size);
          if (!schemaBytes) {
            capturedError = {
              code: 'INTERNAL',
              message: 'Received batch before schema',
              retryable: false,
            };
            break;
          }
          // Combine schema + batch bytes using reusable buffer
          try {
            const combined = combineForParsing(schemaBytes, bytes);
            const reader = await RecordBatchReader.from(combined);
            for await (const batch of reader) {
              yield { type: 'batch', batch };
            }
          } catch (e) {
            capturedError = {
              code: 'INTERNAL',
              message: `Failed to parse batch: ${e}`,
              retryable: false,
            };
          }
          break;
        }
        case 'done': {
          // Defensive: don't yield done if we captured an error earlier
          if (!capturedError) {
            yield { type: 'done' };
            return;
          }
          break;
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
