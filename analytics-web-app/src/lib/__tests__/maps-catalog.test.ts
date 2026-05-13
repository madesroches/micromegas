import {
  formatMapName,
  normalizeMapFilename,
  resolveMapBlobUrl,
  fetchMapCatalog,
  __resetMapCatalogForTest,
} from '../maps-catalog'

describe('normalizeMapFilename', () => {
  it('strips legacy /maps/ prefix', () => {
    expect(normalizeMapFilename('/maps/main.glb')).toBe('main.glb')
  })

  it('leaves bare filenames alone', () => {
    expect(normalizeMapFilename('main.glb')).toBe('main.glb')
  })

  it('returns undefined for empty/missing input', () => {
    expect(normalizeMapFilename(undefined)).toBeUndefined()
    expect(normalizeMapFilename(null as unknown as undefined)).toBeUndefined()
    expect(normalizeMapFilename('')).toBeUndefined()
  })
})

describe('resolveMapBlobUrl', () => {
  it('composes the blob URL from a bare filename', () => {
    expect(resolveMapBlobUrl('main.glb', '/mmlocal')).toBe('/mmlocal/api/maps/blob/main.glb')
  })

  it('strips a legacy /maps/ prefix before composing', () => {
    expect(resolveMapBlobUrl('/maps/main.glb', '/mmlocal')).toBe('/mmlocal/api/maps/blob/main.glb')
  })

  it('returns undefined for empty input', () => {
    expect(resolveMapBlobUrl(undefined, '/mmlocal')).toBeUndefined()
    expect(resolveMapBlobUrl('', '/mmlocal')).toBeUndefined()
  })

  it('works with empty base path (root deployment)', () => {
    expect(resolveMapBlobUrl('main.glb', '')).toBe('/api/maps/blob/main.glb')
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
