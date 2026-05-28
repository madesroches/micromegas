import {
  formatMapName,
  mapFileBasename,
  resolveMapBlobUrl,
  fetchMapCatalog,
  __resetMapCatalogForTest,
  invalidateMapCatalog,
  uploadMap,
  deleteMap,
  MapApiError,
} from '../maps-catalog'

describe('resolveMapBlobUrl', () => {
  it('composes the blob URL from a bare filename', () => {
    expect(resolveMapBlobUrl('main.glb', '/mmlocal')).toBe('/mmlocal/api/maps/blob/main.glb')
  })

  it('returns undefined for empty input', () => {
    expect(resolveMapBlobUrl(undefined, '/mmlocal')).toBeUndefined()
    expect(resolveMapBlobUrl('', '/mmlocal')).toBeUndefined()
  })

  it('works with empty base path (root deployment)', () => {
    expect(resolveMapBlobUrl('main.glb', '')).toBe('/api/maps/blob/main.glb')
  })

  it('reduces a path-prefixed legacy value to its basename', () => {
    // No double slash, single trailing segment — matches the backend's
    // single-segment blob route.
    expect(resolveMapBlobUrl('/maps/Arena_North.glb', '/micromegas')).toBe(
      '/micromegas/api/maps/blob/Arena_North.glb'
    )
    expect(resolveMapBlobUrl('maps/Arena_North.glb', '')).toBe('/api/maps/blob/Arena_North.glb')
  })

  it('returns undefined when the value has no filename segment', () => {
    expect(resolveMapBlobUrl('maps/', '/mmlocal')).toBeUndefined()
  })
})

describe('mapFileBasename', () => {
  it('passes a bare filename through unchanged', () => {
    expect(mapFileBasename('main.glb')).toBe('main.glb')
  })

  it('strips a path prefix', () => {
    expect(mapFileBasename('/maps/Arena_North.glb')).toBe('Arena_North.glb')
    expect(mapFileBasename('maps/Arena_North.glb')).toBe('Arena_North.glb')
  })

  it('returns undefined for empty or filename-less input', () => {
    expect(mapFileBasename(undefined)).toBeUndefined()
    expect(mapFileBasename('')).toBeUndefined()
    expect(mapFileBasename('maps/')).toBeUndefined()
  })
})

describe('formatMapName', () => {
  it('strips .glb', () => {
    expect(formatMapName('main.glb')).toBe('main')
  })

  it('preserves underscores', () => {
    expect(formatMapName('level_a.glb')).toBe('level_a')
    expect(formatMapName('Arena_North_01_WP.glb')).toBe('Arena_North_01_WP')
  })
})

describe('fetchMapCatalog', () => {
  beforeEach(() => {
    __resetMapCatalogForTest()
  })

  afterEach(() => {
    jest.restoreAllMocks()
  })

  it('caches the result across calls (single fetch)', async () => {
    const fetchMock = jest.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve([{ file: 'main.glb', size: 100 }]),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    const first = await fetchMapCatalog('/mmlocal')
    const second = await fetchMapCatalog('/mmlocal')

    expect(first).toEqual([{ file: 'main.glb', size: 100 }])
    expect(second).toEqual(first)
    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(fetchMock).toHaveBeenCalledWith(
      '/mmlocal/api/maps/catalog',
      expect.objectContaining({ credentials: 'include' })
    )
  })

  it('returns an empty array on fetch error', async () => {
    global.fetch = jest.fn().mockRejectedValue(new Error('network')) as unknown as typeof fetch

    const catalog = await fetchMapCatalog('/mmlocal')
    expect(catalog).toEqual([])
  })

  it('returns an empty array on non-OK response', async () => {
    global.fetch = jest.fn().mockResolvedValue({
      ok: false,
      json: () => Promise.resolve(null),
    } as unknown as Response) as unknown as typeof fetch

    const catalog = await fetchMapCatalog('/mmlocal')
    expect(catalog).toEqual([])
  })
})

describe('uploadMap', () => {
  beforeEach(() => {
    __resetMapCatalogForTest()
  })

  afterEach(() => {
    jest.restoreAllMocks()
  })

  it('PUTs the file as raw body with model/gltf-binary and returns the parsed body', async () => {
    const fetchMock = jest.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve({ file: 'level.glb', size: 1234 }),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    const file = new File([new Uint8Array([1, 2, 3, 4])], 'level.glb', {
      type: 'model/gltf-binary',
    })
    const result = await uploadMap(file, '/mmlocal')

    expect(result).toEqual({ file: 'level.glb', size: 1234 })
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/mmlocal/api/maps/blob/level.glb')
    expect(init).toEqual(
      expect.objectContaining({
        method: 'PUT',
        body: file,
        credentials: 'include',
        headers: expect.objectContaining({
          'Content-Type': 'model/gltf-binary',
        }),
      })
    )
  })

  it('invalidates the catalog cache on success', async () => {
    // Prime the cache with a single fetch
    const catalogPayload = [{ file: 'main.glb', size: 100, last_modified: '2026-01-01T00:00:00Z' }]
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve(catalogPayload),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({ file: 'main.glb', size: 50 }),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () =>
          Promise.resolve([
            ...catalogPayload,
            { file: 'new.glb', size: 1, last_modified: '2026-01-02T00:00:00Z' },
          ]),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    const first = await fetchMapCatalog('/mmlocal')
    expect(first).toHaveLength(1)
    await uploadMap(new File([new Uint8Array(1)], 'main.glb'), '/mmlocal')
    const second = await fetchMapCatalog('/mmlocal')
    expect(second).toHaveLength(2)
  })

  it('surfaces JSON server errors as MapApiError with code + message', async () => {
    global.fetch = jest.fn().mockResolvedValue({
      ok: false,
      status: 415,
      json: () =>
        Promise.resolve({
          code: 'UNSUPPORTED_MEDIA_TYPE',
          message: 'Content-Type must be model/gltf-binary',
        }),
    } as unknown as Response) as unknown as typeof fetch

    const file = new File([new Uint8Array(8)], 'bad.glb', { type: 'text/plain' })
    await expect(uploadMap(file, '/mmlocal')).rejects.toMatchObject({
      code: 'UNSUPPORTED_MEDIA_TYPE',
      message: 'Content-Type must be model/gltf-binary',
      status: 415,
    })
  })

  it('maps 413 (axum body-limit plaintext) to a friendly TOO_LARGE error', async () => {
    // DefaultBodyLimit responds with plaintext, so reading JSON would
    // throw — uploadMap short-circuits on 413 with a stable code.
    global.fetch = jest.fn().mockResolvedValue({
      ok: false,
      status: 413,
      json: () => Promise.reject(new Error('not json')),
    } as unknown as Response) as unknown as typeof fetch

    const file = new File([new Uint8Array(8)], 'big.glb', { type: 'model/gltf-binary' })
    await expect(uploadMap(file, '/mmlocal')).rejects.toMatchObject({
      code: 'TOO_LARGE',
      status: 413,
    })
  })

  it('falls back to HTTP status when the error body is not JSON', async () => {
    global.fetch = jest.fn().mockResolvedValue({
      ok: false,
      status: 500,
      json: () => Promise.reject(new Error('not json')),
    } as unknown as Response) as unknown as typeof fetch

    const file = new File([new Uint8Array(1)], 'x.glb', { type: 'model/gltf-binary' })
    await expect(uploadMap(file, '/mmlocal')).rejects.toBeInstanceOf(MapApiError)
  })
})

describe('deleteMap', () => {
  beforeEach(() => {
    __resetMapCatalogForTest()
  })

  afterEach(() => {
    jest.restoreAllMocks()
  })

  it('DELETEs the right URL and invalidates the cache on success', async () => {
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        json: () =>
          Promise.resolve([{ file: 'main.glb', size: 1, last_modified: '2026-01-01T00:00:00Z' }]),
      } as unknown as Response)
      .mockResolvedValueOnce({ ok: true, status: 204 } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve([]),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await fetchMapCatalog('/mmlocal')
    await deleteMap('main.glb', '/mmlocal')
    const after = await fetchMapCatalog('/mmlocal')

    expect(after).toEqual([])
    const deleteCall = fetchMock.mock.calls[1]
    expect(deleteCall[0]).toBe('/mmlocal/api/maps/blob/main.glb')
    expect(deleteCall[1]).toEqual(
      expect.objectContaining({ method: 'DELETE', credentials: 'include' })
    )
  })

  it('surfaces server errors as MapApiError', async () => {
    global.fetch = jest.fn().mockResolvedValue({
      ok: false,
      status: 403,
      json: () =>
        Promise.resolve({ code: 'FORBIDDEN', message: 'Admin access required' }),
    } as unknown as Response) as unknown as typeof fetch

    await expect(deleteMap('main.glb', '/mmlocal')).rejects.toMatchObject({
      code: 'FORBIDDEN',
      status: 403,
    })
  })
})

describe('invalidateMapCatalog', () => {
  beforeEach(() => {
    __resetMapCatalogForTest()
  })

  afterEach(() => {
    jest.restoreAllMocks()
  })

  it('forces a re-fetch on the next fetchMapCatalog call', async () => {
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve([{ file: 'main.glb', size: 1 }]),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve([{ file: 'other.glb', size: 2 }]),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await fetchMapCatalog('/mmlocal')
    invalidateMapCatalog()
    await fetchMapCatalog('/mmlocal')
    expect(fetchMock).toHaveBeenCalledTimes(2)
  })
})
