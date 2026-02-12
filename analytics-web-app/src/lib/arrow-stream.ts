/**
 * Arrow IPC streaming client for query endpoint
 *
 * Implements a JSON-framed protocol parser that streams Arrow IPC data:
 * - {"type":"schema","size":N}\n followed by N bytes of schema IPC
 * - {"type":"batch","size":N,"rows":M}\n followed by N bytes of batch IPC
 * - {"type":"done"}\n on success
 * - {"type":"error","code":"..","message":".."}\n on error
 */

import { RecordBatch, RecordBatchReader, Schema } from 'apache-arrow';
import { authenticatedFetch, AuthenticationError, getApiBase } from './api';

export type ErrorCode = 'INVALID_SQL' | 'CONNECTION_FAILED' | 'INTERNAL' | 'FORBIDDEN';

interface DataHeader {
  type: 'schema' | 'batch';
  size: number;
  rows?: number;
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
  dataSource?: string;
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
  const response = await authenticatedFetch(`${getApiBase()}/query-stream`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      sql: params.sql,
      params: params.params || {},
      begin: params.begin,
      end: params.end,
      data_source: params.dataSource || '',
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

  // Use a queue-based async generator to feed bytes to Arrow's RecordBatchReader.
  // The reader maintains dictionary state internally, handling dictionary-encoded
  // columns correctly across batches.
  const byteQueue: Uint8Array[] = [];
  let queueResolver: ((value: void) => void) | null = null;
  let queueDone = false;
  let capturedError: StreamError | null = null;

  // Async generator that yields IPC bytes to RecordBatchReader
  async function* ipcByteStream(): AsyncGenerator<Uint8Array> {
    while (true) {
      while (byteQueue.length > 0) {
        yield byteQueue.shift()!;
      }
      if (queueDone) return;
      // Wait for more bytes
      await new Promise<void>((resolve) => {
        queueResolver = resolve;
      });
      queueResolver = null;
    }
  }

  const pushBytes = (bytes: Uint8Array) => {
    byteQueue.push(bytes);
    queueResolver?.();
  };

  const endByteStream = () => {
    queueDone = true;
    queueResolver?.();
  };

  try {
    // Start the Arrow reader consuming from our byte stream
    const readerPromise = RecordBatchReader.from(ipcByteStream());

    // Parse frames and push bytes to the queue
    const parseFrames = async () => {
      // eslint-disable-next-line no-constant-condition
      while (true) {
        const line = await bufferedReader.readLine();
        if (line === null) break;

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
            pushBytes(bytes);
            break;
          }
          case 'done':
            endByteStream();
            return 'done';
          case 'error':
            capturedError = {
              code: frame.code,
              message: frame.message,
              retryable: isRetryable(frame.code),
            };
            endByteStream();
            return 'error';
        }
      }
      endByteStream();
      return capturedError ? 'error' : 'done';
    };

    // Run frame parsing in parallel with batch consumption
    const frameParsePromise = parseFrames();

    // Wait for reader to be ready and get schema
    const reader = await readerPromise;
    if (reader.schema) {
      yield { type: 'schema', schema: reader.schema };
    }

    // Yield batches as they arrive
    for await (const batch of reader) {
      yield { type: 'batch', batch };
    }

    // Wait for frame parsing to complete
    const result = await frameParsePromise;

    if (capturedError) {
      yield { type: 'error', error: capturedError };
    } else if (result === 'done') {
      yield { type: 'done' };
    }
  } finally {
    bufferedReader.release();
  }
}

export interface FetchProgress {
  bytes: number;
  rows: number;
}

/**
 * Fetches query results as raw Arrow IPC stream bytes.
 * Returns a complete IPC stream (schema + batches + EOS) suitable for
 * passing directly to WASM DataFusion's register_table.
 */
export async function fetchQueryIPC(
  params: StreamQueryParams,
  signal?: AbortSignal,
  onProgress?: (progress: FetchProgress) => void,
): Promise<Uint8Array> {
  const response = await authenticatedFetch(`${getApiBase()}/query-stream`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      sql: params.sql,
      params: params.params || {},
      begin: params.begin,
      end: params.end,
      data_source: params.dataSource || '',
    }),
    signal,
  });

  if (!response.ok) {
    if (response.status === 401) {
      throw new AuthenticationError();
    }
    const text = await response.text();
    if (response.status === 403) {
      try {
        const frame = JSON.parse(text.trim()) as ErrorFrame;
        throw new Error(frame.message);
      } catch (e) {
        if (e instanceof SyntaxError) {
          throw new Error(`HTTP 403: ${text}`);
        }
        throw e;
      }
    }
    throw new Error(`HTTP ${response.status}: ${text}`);
  }

  if (!response.body) {
    throw new Error('No response body');
  }

  const bufferedReader = new BufferedReader(response.body.getReader());

  try {
    // Collect all IPC message bytes from the framed protocol
    const ipcChunks: Uint8Array[] = [];
    let totalSize = 0;
    let totalRows = 0;

    // eslint-disable-next-line no-constant-condition
    while (true) {
      const line = await bufferedReader.readLine();
      if (line === null) break;

      let frame: Frame;
      try {
        frame = JSON.parse(line);
      } catch {
        throw new Error(`Invalid frame: ${line.slice(0, 100)}`);
      }

      switch (frame.type) {
        case 'schema':
        case 'batch': {
          const bytes = await bufferedReader.readBytes(frame.size);
          ipcChunks.push(bytes);
          totalSize += bytes.length;
          if (onProgress && frame.type === 'batch') {
            totalRows += frame.rows ?? 0;
            onProgress({ bytes: totalSize, rows: totalRows });
          }
          break;
        }
        case 'done': {
          // Append EOS: continuation marker (0xFFFFFFFF) + 0 metadata length
          const eos = new Uint8Array(8);
          const view = new DataView(eos.buffer);
          view.setInt32(0, -1, true);  // 0xFFFFFFFF continuation
          view.setInt32(4, 0, true);   // 0 metadata length = EOS
          ipcChunks.push(eos);
          totalSize += eos.length;

          // Assemble into single Uint8Array
          const result = new Uint8Array(totalSize);
          let offset = 0;
          for (const chunk of ipcChunks) {
            result.set(chunk, offset);
            offset += chunk.length;
          }
          return result;
        }
        case 'error':
          throw new Error(frame.message);
      }
    }

    throw new Error('Stream ended without done frame');
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
