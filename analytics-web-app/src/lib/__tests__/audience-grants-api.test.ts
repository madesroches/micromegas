/**
 * `audience-grants-api.ts` (#1510): decoding `GET .../visible`, create/delete status handling,
 * error surfacing, and `validateSelector`'s byte bound.
 */
import {
  AudienceGrantError,
  MAX_SELECTOR_BYTES,
  createAudienceGrant,
  deleteAudienceGrant,
  fetchMyAudiences,
  fetchVisibleGrants,
  validateSelector,
} from '../audience-grants-api'

function mockFetch(response: { ok: boolean; status: number; json: () => unknown }) {
  const fetchMock = vi.fn().mockResolvedValue({
    ok: response.ok,
    status: response.status,
    json: () => Promise.resolve(response.json()),
  } as unknown as Response)
  global.fetch = fetchMock as unknown as typeof fetch
  return fetchMock
}

describe('audience-grants-api', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('fetchVisibleGrants decodes a JSON array to AudienceGrant[]', async () => {
    mockFetch({
      ok: true,
      status: 200,
      json: () => [
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'group:eng',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
        },
      ],
    })

    const grants = await fetchVisibleGrants()
    expect(grants).toEqual([
      {
        audience: 'team-alpha',
        axis: 'read',
        selector: 'group:eng',
        createdAt: new Date('2026-08-14T00:00:00Z'),
        createdBy: 'admin@example.com',
      },
    ])
  })

  it('fetchVisibleGrants surfaces a non-2xx as AudienceGrantError', async () => {
    mockFetch({
      ok: false,
      status: 403,
      json: () => ({ code: 'FORBIDDEN', message: 'self-service grant management is disabled' }),
    })

    await expect(fetchVisibleGrants()).rejects.toMatchObject({
      name: 'AudienceGrantError',
      code: 'FORBIDDEN',
      status: 403,
      message: 'self-service grant management is disabled',
    })
  })

  it('createAudienceGrant reports created: true on 201', async () => {
    const fetchMock = mockFetch({
      ok: true,
      status: 201,
      json: () => ({
        audience: 'team-alpha',
        axis: 'read',
        selector: 'group:eng',
        created_at: '2026-08-14T00:00:00Z',
        created_by: 'admin@example.com',
      }),
    })

    const { created, grant } = await createAudienceGrant('team-alpha', 'read', 'group:eng')
    expect(created).toBe(true)
    expect(grant.selector).toBe('group:eng')
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/audience-grants')
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      audience: 'team-alpha',
      axis: 'read',
      selector: 'group:eng',
    })
  })

  it('createAudienceGrant reports created: false on 200 (already existed)', async () => {
    mockFetch({
      ok: true,
      status: 200,
      json: () => ({
        audience: 'team-alpha',
        axis: 'read',
        selector: 'group:eng',
        created_at: '2026-08-14T00:00:00Z',
        created_by: 'admin@example.com',
      }),
    })

    const { created } = await createAudienceGrant('team-alpha', 'read', 'group:eng')
    expect(created).toBe(false)
  })

  it('createAudienceGrant surfaces a 403 as AudienceGrantError', async () => {
    mockFetch({
      ok: false,
      status: 403,
      json: () => ({
        code: 'FORBIDDEN',
        message: "you have no read grant on team-alpha to share",
      }),
    })

    await expect(createAudienceGrant('team-alpha', 'read', 'user:x@example.com')).rejects.toMatchObject(
      { code: 'FORBIDDEN', status: 403 }
    )
  })

  it('deleteAudienceGrant encodes every component and resolves on 204', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await expect(
      deleteAudienceGrant('team alpha', 'read', 'group:eng/leads')
    ).resolves.toBeUndefined()

    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe(
      '/api/audience-grants?audience=team%20alpha&axis=read&selector=group%3Aeng%2Fleads'
    )
    expect((init as RequestInit).method).toBe('DELETE')
  })

  it('deleteAudienceGrant surfaces a 404 as AudienceGrantError', async () => {
    mockFetch({ ok: false, status: 404, json: () => ({ code: 'NOT_FOUND', message: 'grant not found' }) })

    await expect(deleteAudienceGrant('team-alpha', 'read', 'group:eng')).rejects.toMatchObject({
      code: 'NOT_FOUND',
      status: 404,
      message: 'grant not found',
    })
  })

  it('fetchMyAudiences surfaces a 403 (knob off) as AudienceGrantError', async () => {
    mockFetch({
      ok: false,
      status: 403,
      json: () => ({ code: 'FORBIDDEN', message: 'self-service minting is disabled' }),
    })

    await expect(fetchMyAudiences()).rejects.toBeInstanceOf(AudienceGrantError)
  })

  describe('validateSelector', () => {
    it('accepts *, user:<id>, and group:<id>', () => {
      expect(validateSelector('*')).toBeNull()
      expect(validateSelector('user:alice@example.com')).toBeNull()
      expect(validateSelector('group:eng')).toBeNull()
    })

    it('rejects an unrecognized prefix', () => {
      expect(validateSelector('everyone')).not.toBeNull()
    })

    it('rejects an empty id after the prefix', () => {
      expect(validateSelector('user:')).not.toBeNull()
      expect(validateSelector('group:')).not.toBeNull()
    })

    it('enforces the 255-byte bound, counted in bytes not chars', () => {
      const longAscii = `user:${'a'.repeat(MAX_SELECTOR_BYTES)}`
      expect(validateSelector(longAscii)).not.toBeNull()

      const fitsAscii = `user:${'a'.repeat(MAX_SELECTOR_BYTES - 'user:'.length)}`
      expect(validateSelector(fitsAscii)).toBeNull()

      // Multi-byte characters must be counted by UTF-8 byte length, not `.length`.
      const multiByte = `user:${'é'.repeat(MAX_SELECTOR_BYTES - 'user:'.length)}`
      expect(validateSelector(multiByte)).not.toBeNull()
    })
  })
})
