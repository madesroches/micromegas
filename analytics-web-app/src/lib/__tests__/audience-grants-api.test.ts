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
  normalizeKeyId,
  validateGrantAudience,
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
          key_name: null,
          key_revoked: false,
        },
        {
          audience: 'team-alpha',
          axis: 'read',
          selector: 'key:0b9f5d0e-5c1a-4a52-9a6e-6f1d7b0c1a11',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'admin@example.com',
          key_name: 'reporter',
          key_revoked: true,
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
        keyName: null,
        keyRevoked: false,
      },
      {
        audience: 'team-alpha',
        axis: 'read',
        selector: 'key:0b9f5d0e-5c1a-4a52-9a6e-6f1d7b0c1a11',
        createdAt: new Date('2026-08-14T00:00:00Z'),
        createdBy: 'admin@example.com',
        keyName: 'reporter',
        keyRevoked: true,
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

  it('fetchMyAudiences decodes held_pairs alongside the existing fields', async () => {
    mockFetch({
      ok: true,
      status: 200,
      json: () => ({
        is_admin: false,
        audiences: ['team-alpha'],
        mint_prefix: 'reader-',
        email: 'reader@example.com',
        held_pairs: ['team-alpha:read', 'team-alpha:mint'],
      }),
    })

    const result = await fetchMyAudiences()
    expect(result.held_pairs).toEqual(['team-alpha:read', 'team-alpha:mint'])
  })

  describe('validateSelector', () => {
    it('accepts *, user:<id>, and group:<id>', () => {
      expect(validateSelector('*')).toBeNull()
      expect(validateSelector('user:alice@example.com')).toBeNull()
      expect(validateSelector('group:eng')).toBeNull()
    })

    it('accepts only a canonical key:<uuid>', () => {
      expect(validateSelector('key:0b9f5d0e-5c1a-4a52-9a6e-6f1d7b0c1a11')).toBeNull()
      expect(validateSelector('key:0B9F5D0E-5C1A-4A52-9A6E-6F1D7B0C1A11')).not.toBeNull()
      expect(validateSelector('key:0b9f5d0e5c1a4a529a6e6f1d7b0c1a11')).not.toBeNull()
      expect(validateSelector('key:')).not.toBeNull()
      expect(validateSelector('key:nope')).not.toBeNull()
    })

    it('normalizeKeyId trims and lowercases', () => {
      expect(normalizeKeyId('  0B9F5D0E-5C1A-4A52-9A6E-6F1D7B0C1A11 ')).toBe(
        '0b9f5d0e-5c1a-4a52-9a6e-6f1d7b0c1a11'
      )
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

  describe('validateGrantAudience', () => {
    it('shows no error for an empty audience', () => {
      expect(validateGrantAudience('', 'read', '*')).toBeNull()
    })

    it('accepts a plain name and the * audience on read', () => {
      expect(validateGrantAudience('team-alpha', 'mint', '*')).toBeNull()
      expect(validateGrantAudience('*', 'read', 'user:a@example.com')).toBeNull()
    })

    it('rejects a name outside the charset, including partial wildcards', () => {
      expect(validateGrantAudience('team alpha', 'read', '*')).not.toBeNull()
      expect(validateGrantAudience('team-*', 'read', 'user:a')).not.toBeNull()
      expect(validateGrantAudience('**', 'read', 'user:a')).not.toBeNull()
    })

    it('rejects * on the mint axis', () => {
      expect(validateGrantAudience('*', 'mint', 'user:a')).toMatch(/read-axis only/)
    })

    it('rejects the * audience with the * selector', () => {
      expect(validateGrantAudience('*', 'read', '*')).toMatch(/everyone selector/)
    })
  })
})
