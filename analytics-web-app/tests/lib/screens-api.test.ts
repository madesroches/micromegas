/**
 * Tests for screens API client
 */
import { authenticatedFetch } from '@/lib/api'

// Mock the dependencies
jest.mock('@/lib/api', () => ({
  authenticatedFetch: jest.fn(),
  getApiBase: () => '/api',
}))

const mockedFetch = authenticatedFetch as jest.MockedFunction<typeof authenticatedFetch>

// Import after mocking
import {
  getScreenTypes,
  getDefaultConfig,
  listScreens,
  getScreen,
  createScreen,
  updateScreen,
  deleteScreen,
  ScreenApiError,
  type Screen,
  type ScreenTypeInfo,
  type ScreenConfig,
} from '@/lib/screens-api'

// Helper to create mock responses
function createMockResponse<T>(data: T, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : 'Error',
    json: async () => data,
    text: async () => JSON.stringify(data),
  } as Response
}

function createErrorResponse(
  status: number,
  code: string,
  message: string
): Response {
  return {
    ok: false,
    status,
    statusText: 'Error',
    json: async () => ({ code, message }),
  } as Response
}

describe('ScreenApiError', () => {
  it('should create error with code, message, and status', () => {
    const error = new ScreenApiError('TEST_CODE', 'Test message', 400)
    expect(error.code).toBe('TEST_CODE')
    expect(error.message).toBe('Test message')
    expect(error.status).toBe(400)
    expect(error.name).toBe('ScreenApiError')
  })

  it('should be instance of Error', () => {
    const error = new ScreenApiError('TEST', 'msg', 500)
    expect(error).toBeInstanceOf(Error)
  })
})

describe('getScreenTypes', () => {
  beforeEach(() => {
    jest.resetAllMocks()
  })

  it('should fetch and return screen types', async () => {
    const mockTypes: ScreenTypeInfo[] = [
      { name: 'process_list', display_name: 'Process List', icon: 'list', description: 'Process list' },
      { name: 'metrics', display_name: 'Metrics', icon: 'chart', description: 'Metrics' },
      { name: 'log', display_name: 'Log', icon: 'file', description: 'Logs' },
    ]
    mockedFetch.mockResolvedValue(createMockResponse(mockTypes))

    const result = await getScreenTypes()

    expect(mockedFetch).toHaveBeenCalledWith('/api/screen-types')
    expect(result).toEqual(mockTypes)
  })

  it('should throw ScreenApiError on HTTP error', async () => {
    mockedFetch.mockResolvedValue(
      createErrorResponse(500, 'INTERNAL', 'Server error')
    )

    await expect(getScreenTypes()).rejects.toThrow(ScreenApiError)
    await expect(getScreenTypes()).rejects.toMatchObject({
      code: 'INTERNAL',
      status: 500,
    })
  })
})

describe('getDefaultConfig', () => {
  beforeEach(() => {
    jest.resetAllMocks()
  })

  it('should fetch default config for a screen type', async () => {
    const mockConfig: ScreenConfig = {
      sql: 'SELECT * FROM processes',
      variables: [],
    }
    mockedFetch.mockResolvedValue(createMockResponse(mockConfig))

    const result = await getDefaultConfig('process_list')

    expect(mockedFetch).toHaveBeenCalledWith('/api/screen-types/process_list/default')
    expect(result).toEqual(mockConfig)
  })

  it('should handle invalid screen type', async () => {
    mockedFetch.mockResolvedValue(
      createErrorResponse(400, 'INVALID_SCREEN_TYPE', 'Unknown screen type')
    )

    // @ts-expect-error testing invalid input
    await expect(getDefaultConfig('invalid')).rejects.toThrow(
      ScreenApiError
    )
  })
})

describe('listScreens', () => {
  beforeEach(() => {
    jest.resetAllMocks()
  })

  it('should fetch and return all screens', async () => {
    const mockScreens: Screen[] = [
      {
        name: 'my-screen',
        screen_type: 'log',
        config: { sql: 'SELECT * FROM logs' },
        created_at: '2024-01-01T00:00:00Z',
        updated_at: '2024-01-01T00:00:00Z',
      },
    ]
    mockedFetch.mockResolvedValue(createMockResponse(mockScreens))

    const result = await listScreens()

    expect(mockedFetch).toHaveBeenCalledWith('/api/screens')
    expect(result).toEqual(mockScreens)
  })

  it('should return empty array when no screens exist', async () => {
    mockedFetch.mockResolvedValue(createMockResponse([]))

    const result = await listScreens()

    expect(result).toEqual([])
  })
})

describe('getScreen', () => {
  beforeEach(() => {
    jest.resetAllMocks()
  })

  it('should fetch a screen by name', async () => {
    const mockScreen: Screen = {
      name: 'error-logs',
      screen_type: 'log',
      config: { sql: 'SELECT * FROM log_entries WHERE level = \'ERROR\'' },
      created_by: 'user@example.com',
      created_at: '2024-01-01T00:00:00Z',
      updated_at: '2024-01-02T00:00:00Z',
    }
    mockedFetch.mockResolvedValue(createMockResponse(mockScreen))

    const result = await getScreen('error-logs')

    expect(mockedFetch).toHaveBeenCalledWith('/api/screens/error-logs')
    expect(result).toEqual(mockScreen)
  })

  it('should URL-encode screen names with special characters', async () => {
    const mockScreen: Screen = {
      name: 'test-screen',
      screen_type: 'log',
      config: { sql: 'SELECT 1' },
      created_at: '2024-01-01T00:00:00Z',
      updated_at: '2024-01-01T00:00:00Z',
    }
    mockedFetch.mockResolvedValue(createMockResponse(mockScreen))

    await getScreen('screen/with/slashes')

    expect(mockedFetch).toHaveBeenCalledWith(
      '/api/screens/screen%2Fwith%2Fslashes'
    )
  })

  it('should throw ScreenApiError for not found', async () => {
    mockedFetch.mockResolvedValue(
      createErrorResponse(404, 'NOT_FOUND', 'Screen not found')
    )

    await expect(getScreen('nonexistent')).rejects.toMatchObject({
      code: 'NOT_FOUND',
      status: 404,
    })
  })
})

describe('createScreen', () => {
  beforeEach(() => {
    jest.resetAllMocks()
  })

  it('should create a new screen', async () => {
    const request = {
      name: 'new-screen',
      screen_type: 'log' as const,
      config: { sql: 'SELECT * FROM log_entries' },
    }
    const mockResponse: Screen = {
      ...request,
      created_by: 'user@example.com',
      created_at: '2024-01-01T00:00:00Z',
      updated_at: '2024-01-01T00:00:00Z',
    }
    mockedFetch.mockResolvedValue(createMockResponse(mockResponse))

    const result = await createScreen(request)

    expect(mockedFetch).toHaveBeenCalledWith('/api/screens', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(request),
    })
    expect(result).toEqual(mockResponse)
  })

  it('should throw ScreenApiError for duplicate name', async () => {
    mockedFetch.mockResolvedValue(
      createErrorResponse(400, 'DUPLICATE_NAME', 'Screen name already exists')
    )

    await expect(
      createScreen({
        name: 'existing',
        screen_type: 'log',
        config: { sql: 'SELECT 1' },
      })
    ).rejects.toMatchObject({
      code: 'DUPLICATE_NAME',
      status: 400,
    })
  })

  it('should throw ScreenApiError for invalid screen type', async () => {
    mockedFetch.mockResolvedValue(
      createErrorResponse(400, 'INVALID_SCREEN_TYPE', 'Unknown screen type')
    )

    await expect(
      createScreen({
        name: 'test',
        // @ts-expect-error testing invalid input
        screen_type: 'invalid',
        config: { sql: 'SELECT 1' },
      })
    ).rejects.toMatchObject({
      code: 'INVALID_SCREEN_TYPE',
      status: 400,
    })
  })

  it('should throw ScreenApiError for invalid name format', async () => {
    mockedFetch.mockResolvedValue(
      createErrorResponse(400, 'NAME_TOO_SHORT', 'Screen name must be at least 3 characters')
    )

    await expect(
      createScreen({
        name: 'ab',
        screen_type: 'log',
        config: { sql: 'SELECT 1' },
      })
    ).rejects.toMatchObject({
      code: 'NAME_TOO_SHORT',
      status: 400,
    })
  })
})

describe('updateScreen', () => {
  beforeEach(() => {
    jest.resetAllMocks()
  })

  it('should update an existing screen', async () => {
    const request = {
      config: { sql: 'SELECT * FROM log_entries WHERE level = \'WARN\'' },
    }
    const mockResponse: Screen = {
      name: 'my-screen',
      screen_type: 'log',
      config: request.config,
      created_at: '2024-01-01T00:00:00Z',
      updated_at: '2024-01-02T00:00:00Z',
    }
    mockedFetch.mockResolvedValue(createMockResponse(mockResponse))

    const result = await updateScreen('my-screen', request)

    expect(mockedFetch).toHaveBeenCalledWith('/api/screens/my-screen', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(request),
    })
    expect(result).toEqual(mockResponse)
  })

  it('should URL-encode screen name', async () => {
    const request = { config: { sql: 'SELECT 1' } }
    mockedFetch.mockResolvedValue(
      createMockResponse({
        name: 'test',
        screen_type: 'log',
        config: request.config,
        created_at: '2024-01-01T00:00:00Z',
        updated_at: '2024-01-01T00:00:00Z',
      })
    )

    await updateScreen('screen/name', request)

    expect(mockedFetch).toHaveBeenCalledWith(
      '/api/screens/screen%2Fname',
      expect.any(Object)
    )
  })

  it('should throw ScreenApiError for not found', async () => {
    mockedFetch.mockResolvedValue(
      createErrorResponse(404, 'NOT_FOUND', 'Screen not found')
    )

    await expect(
      updateScreen('nonexistent', { config: { sql: 'SELECT 1' } })
    ).rejects.toMatchObject({
      code: 'NOT_FOUND',
      status: 404,
    })
  })
})

describe('deleteScreen', () => {
  beforeEach(() => {
    jest.resetAllMocks()
  })

  it('should delete a screen', async () => {
    mockedFetch.mockResolvedValue({
      ok: true,
      status: 204,
    } as Response)

    await expect(deleteScreen('my-screen')).resolves.toBeUndefined()

    expect(mockedFetch).toHaveBeenCalledWith('/api/screens/my-screen', {
      method: 'DELETE',
    })
  })

  it('should URL-encode screen name', async () => {
    mockedFetch.mockResolvedValue({
      ok: true,
      status: 204,
    } as Response)

    await deleteScreen('screen/name')

    expect(mockedFetch).toHaveBeenCalledWith(
      '/api/screens/screen%2Fname',
      { method: 'DELETE' }
    )
  })

  it('should throw ScreenApiError for not found', async () => {
    mockedFetch.mockResolvedValue(
      createErrorResponse(404, 'NOT_FOUND', 'Screen not found')
    )

    await expect(deleteScreen('nonexistent')).rejects.toMatchObject({
      code: 'NOT_FOUND',
      status: 404,
    })
  })

  it('should handle error response without JSON body', async () => {
    mockedFetch.mockResolvedValue({
      ok: false,
      status: 500,
      json: async () => {
        throw new Error('Invalid JSON')
      },
    } as Response)

    await expect(deleteScreen('test')).rejects.toMatchObject({
      code: 'UNKNOWN_ERROR',
      status: 500,
    })
  })
})

describe('error handling edge cases', () => {
  beforeEach(() => {
    jest.resetAllMocks()
  })

  it('should handle non-JSON error responses', async () => {
    mockedFetch.mockResolvedValue({
      ok: false,
      status: 500,
      json: async () => {
        throw new Error('Not JSON')
      },
    } as Response)

    await expect(listScreens()).rejects.toMatchObject({
      code: 'UNKNOWN_ERROR',
      message: 'HTTP 500',
      status: 500,
    })
  })

  it('should handle error response with missing code', async () => {
    mockedFetch.mockResolvedValue({
      ok: false,
      status: 400,
      json: async () => ({ message: 'Bad request' }),
    } as Response)

    await expect(listScreens()).rejects.toMatchObject({
      code: 'UNKNOWN_ERROR',
      message: 'Bad request',
      status: 400,
    })
  })

  it('should handle error response with missing message', async () => {
    mockedFetch.mockResolvedValue({
      ok: false,
      status: 400,
      json: async () => ({ code: 'SOME_ERROR' }),
    } as Response)

    await expect(listScreens()).rejects.toMatchObject({
      code: 'SOME_ERROR',
      message: 'HTTP 400',
      status: 400,
    })
  })
})
