/**
 * Tests for Arrow IPC streaming client
 */
import { authenticatedFetch } from '@/lib/api';
import { ErrorCode, StreamResult } from '../arrow-stream';

// We need to import the module after mocking
jest.mock('@/lib/api', () => ({
  authenticatedFetch: jest.fn(),
  AuthenticationError: class AuthenticationError extends Error {
    constructor() {
      super('Authentication required');
      this.name = 'AuthenticationError';
    }
  },
  getApiBase: () => '/api',
}));

const mockedFetch = authenticatedFetch as jest.MockedFunction<typeof authenticatedFetch>;

// Helper to create a mock ReadableStream from data
function createMockStream(chunks: (string | Uint8Array)[]): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  let index = 0;

  return new ReadableStream<Uint8Array>({
    pull(controller) {
      if (index < chunks.length) {
        const chunk = chunks[index++];
        if (typeof chunk === 'string') {
          controller.enqueue(encoder.encode(chunk));
        } else {
          controller.enqueue(chunk);
        }
      } else {
        controller.close();
      }
    },
  });
}

// Helper to create a mock Response
function createMockResponse(
  chunks: (string | Uint8Array)[],
  status = 200,
  statusText = 'OK'
): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText,
    body: createMockStream(chunks),
    headers: new Headers(),
    text: async () => chunks.map(c => typeof c === 'string' ? c : new TextDecoder().decode(c)).join(''),
    json: async () => JSON.parse(chunks.map(c => typeof c === 'string' ? c : new TextDecoder().decode(c)).join('')),
  } as Response;
}

describe('streamQuery', () => {
  beforeEach(() => {
    jest.resetAllMocks();
  });

  describe('error handling', () => {
    it('should handle HTTP 401 by throwing AuthenticationError', async () => {
      const { AuthenticationError } = await import('@/lib/api');
      mockedFetch.mockResolvedValue({
        ok: false,
        status: 401,
        statusText: 'Unauthorized',
      } as Response);

      const { streamQuery } = await import('../arrow-stream');

      await expect(async () => {
        for await (const _ of streamQuery({ sql: 'SELECT 1' })) {
          // consume
        }
      }).rejects.toThrow(AuthenticationError);
    });

    it('should handle HTTP 403 with error frame', async () => {
      const errorFrame = { type: 'error', code: 'FORBIDDEN', message: 'Function not allowed' };
      mockedFetch.mockResolvedValue({
        ok: false,
        status: 403,
        statusText: 'Forbidden',
        text: async () => JSON.stringify(errorFrame),
      } as Response);

      const { streamQuery } = await import('../arrow-stream');

      const results: StreamResult[] = [];
      for await (const result of streamQuery({ sql: 'SELECT retire_partitions()' })) {
        results.push(result);
      }

      expect(results).toHaveLength(1);
      expect(results[0].type).toBe('error');
      if (results[0].type === 'error') {
        expect(results[0].error.code).toBe('FORBIDDEN');
        expect(results[0].error.retryable).toBe(false);
      }
    });

    it('should handle HTTP 500 by throwing', async () => {
      mockedFetch.mockResolvedValue({
        ok: false,
        status: 500,
        statusText: 'Internal Server Error',
        text: async () => 'Internal error',
      } as Response);

      const { streamQuery } = await import('../arrow-stream');

      await expect(async () => {
        for await (const _ of streamQuery({ sql: 'SELECT 1' })) {
          // consume
        }
      }).rejects.toThrow('HTTP 500: Internal error');
    });

    it('should throw if response has no body', async () => {
      mockedFetch.mockResolvedValue({
        ok: true,
        status: 200,
        body: null,
      } as Response);

      const { streamQuery } = await import('../arrow-stream');

      await expect(async () => {
        for await (const _ of streamQuery({ sql: 'SELECT 1' })) {
          // consume
        }
      }).rejects.toThrow('No response body');
    });
  });

  describe('frame parsing', () => {
    it('should yield error for invalid JSON frame', async () => {
      mockedFetch.mockResolvedValue(createMockResponse([
        'not valid json\n',
      ]));

      const { streamQuery } = await import('../arrow-stream');

      const results: StreamResult[] = [];
      for await (const result of streamQuery({ sql: 'SELECT 1' })) {
        results.push(result);
      }

      expect(results).toHaveLength(1);
      expect(results[0].type).toBe('error');
      if (results[0].type === 'error') {
        expect(results[0].error.code).toBe('INTERNAL');
        expect(results[0].error.message).toContain('Invalid frame');
      }
    });

    it('should yield done frame and complete', async () => {
      mockedFetch.mockResolvedValue(createMockResponse([
        '{"type":"done"}\n',
      ]));

      const { streamQuery } = await import('../arrow-stream');

      const results: StreamResult[] = [];
      for await (const result of streamQuery({ sql: 'SELECT 1' })) {
        results.push(result);
      }

      expect(results).toHaveLength(1);
      expect(results[0].type).toBe('done');
    });

    it('should yield error frame with correct retryable flag for CONNECTION_FAILED', async () => {
      mockedFetch.mockResolvedValue(createMockResponse([
        '{"type":"error","code":"CONNECTION_FAILED","message":"Cannot connect"}\n',
      ]));

      const { streamQuery } = await import('../arrow-stream');

      const results: StreamResult[] = [];
      for await (const result of streamQuery({ sql: 'SELECT 1' })) {
        results.push(result);
      }

      expect(results).toHaveLength(1);
      expect(results[0].type).toBe('error');
      if (results[0].type === 'error') {
        expect(results[0].error.code).toBe('CONNECTION_FAILED');
        expect(results[0].error.retryable).toBe(true);
      }
    });

    it('should yield error frame with correct retryable flag for INVALID_SQL', async () => {
      mockedFetch.mockResolvedValue(createMockResponse([
        '{"type":"error","code":"INVALID_SQL","message":"Syntax error"}\n',
      ]));

      const { streamQuery } = await import('../arrow-stream');

      const results: StreamResult[] = [];
      for await (const result of streamQuery({ sql: 'SELECT' })) {
        results.push(result);
      }

      expect(results).toHaveLength(1);
      expect(results[0].type).toBe('error');
      if (results[0].type === 'error') {
        expect(results[0].error.code).toBe('INVALID_SQL');
        expect(results[0].error.retryable).toBe(false);
      }
    });
  });

  describe('request parameters', () => {
    it('should send correct request body', async () => {
      mockedFetch.mockResolvedValue(createMockResponse(['{"type":"done"}\n']));

      const { streamQuery } = await import('../arrow-stream');

      for await (const _ of streamQuery({
        sql: 'SELECT * FROM logs WHERE level = $level',
        params: { level: 'ERROR' },
        begin: '2024-01-01T00:00:00Z',
        end: '2024-01-02T00:00:00Z',
      })) {
        // consume
      }

      expect(mockedFetch).toHaveBeenCalledTimes(1);
      const [url, options] = mockedFetch.mock.calls[0];
      expect(url).toBe('/api/query-stream');
      expect(options?.method).toBe('POST');
      expect(options?.headers).toEqual({ 'Content-Type': 'application/json' });

      const body = JSON.parse(options?.body as string);
      expect(body.sql).toBe('SELECT * FROM logs WHERE level = $level');
      expect(body.params).toEqual({ level: 'ERROR' });
      expect(body.begin).toBe('2024-01-01T00:00:00Z');
      expect(body.end).toBe('2024-01-02T00:00:00Z');
    });

    it('should pass abort signal', async () => {
      mockedFetch.mockResolvedValue(createMockResponse(['{"type":"done"}\n']));

      const { streamQuery } = await import('../arrow-stream');

      const controller = new AbortController();
      for await (const _ of streamQuery({ sql: 'SELECT 1' }, controller.signal)) {
        // consume
      }

      expect(mockedFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({ signal: controller.signal })
      );
    });
  });
});

describe('executeStreamQuery', () => {
  beforeEach(() => {
    jest.resetAllMocks();
  });

  it('should collect all results', async () => {
    mockedFetch.mockResolvedValue(createMockResponse([
      '{"type":"done"}\n',
    ]));

    const { executeStreamQuery } = await import('../arrow-stream');

    const result = await executeStreamQuery({ sql: 'SELECT 1' });

    expect(result.error).toBeNull();
  });

  it('should return error from error frame', async () => {
    mockedFetch.mockResolvedValue(createMockResponse([
      '{"type":"error","code":"INVALID_SQL","message":"Bad query"}\n',
    ]));

    const { executeStreamQuery } = await import('../arrow-stream');

    const result = await executeStreamQuery({ sql: 'SELECT' });

    expect(result.error).not.toBeNull();
    expect(result.error?.code).toBe('INVALID_SQL');
    expect(result.error?.message).toBe('Bad query');
  });
});

describe('error code retryability', () => {
  const testCases: Array<{ code: ErrorCode; retryable: boolean }> = [
    { code: 'INVALID_SQL', retryable: false },
    { code: 'CONNECTION_FAILED', retryable: true },
    { code: 'INTERNAL', retryable: false },
    { code: 'FORBIDDEN', retryable: false },
  ];

  beforeEach(() => {
    jest.resetAllMocks();
  });

  testCases.forEach(({ code, retryable }) => {
    it(`should mark ${code} as ${retryable ? 'retryable' : 'not retryable'}`, async () => {
      mockedFetch.mockResolvedValue(createMockResponse([
        `{"type":"error","code":"${code}","message":"test"}\n`,
      ]));

      const { streamQuery } = await import('../arrow-stream');

      const results: StreamResult[] = [];
      for await (const result of streamQuery({ sql: 'SELECT 1' })) {
        results.push(result);
      }

      expect(results[0].type).toBe('error');
      if (results[0].type === 'error') {
        expect(results[0].error.retryable).toBe(retryable);
      }
    });
  });
});
