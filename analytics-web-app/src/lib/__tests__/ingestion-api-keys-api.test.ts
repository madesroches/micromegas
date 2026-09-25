/**
 * `listIngestionApiKeys` must send the caller's `limit` explicitly rather than
 * omitting it (which falls back to the server's lower `DEFAULT_LIMIT` of
 * 100) — omitting it silently truncates the list on any deployment with more
 * than 100 lifetime keys, with no indication anything is missing. The page
 * passes `MAX_INGESTION_API_KEYS_LIST_LIMIT` (the server's max), so that's what
 * goes on the wire. `offset` must be threaded through too, so the page can
 * page past the first 500 lifetime keys.
 */
import {
  listIngestionApiKeys,
  mintIngestionApiKey,
  setIngestionApiKeyAllowlist,
  MAX_INGESTION_API_KEYS_LIST_LIMIT,
} from '../ingestion-api-keys-api'

describe('ingestion-api-keys-api', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('listIngestionApiKeys sends the given limit and offset 0 on the first page', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve([]),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await listIngestionApiKeys(true, 0, MAX_INGESTION_API_KEYS_LIST_LIMIT)

    const [url] = fetchMock.mock.calls[0]
    expect(url).toBe(
      `/api/ingestion-api-keys?limit=${MAX_INGESTION_API_KEYS_LIST_LIMIT}&offset=0&include_revoked=true`
    )
    // The page's default page size is the server's own `MAX_LIMIT`.
    expect(MAX_INGESTION_API_KEYS_LIST_LIMIT).toBe(500)
  })

  it('listIngestionApiKeys forwards a non-zero offset', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve([]),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await listIngestionApiKeys(true, MAX_INGESTION_API_KEYS_LIST_LIMIT, MAX_INGESTION_API_KEYS_LIST_LIMIT)

    const [url] = fetchMock.mock.calls[0]
    expect(url).toBe(
      `/api/ingestion-api-keys?limit=${MAX_INGESTION_API_KEYS_LIST_LIMIT}&offset=${MAX_INGESTION_API_KEYS_LIST_LIMIT}&include_revoked=true`
    )
  })

  it('mintIngestionApiKey puts allowed_cidrs in the POST body', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () =>
        Promise.resolve({ key_id: 'key-1', name: 'n', created_at: 't', key: 'k', audience: 'public' }),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await mintIngestionApiKey('n', { audience: 'public', allowed_cidrs: ['127.0.0.1'] })

    const [, init] = fetchMock.mock.calls[0]
    expect(JSON.parse(init.body)).toEqual({
      name: 'n',
      audience: 'public',
      allowed_cidrs: ['127.0.0.1'],
    })
  })

  it('setIngestionApiKeyAllowlist sends PATCH with allowed_cidrs, including []', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve({ allowed_cidrs: [] }),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await setIngestionApiKeyAllowlist('key with spaces', [])

    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/ingestion-api-keys/key%20with%20spaces/allowlist')
    expect(init.method).toBe('PATCH')
    expect(JSON.parse(init.body)).toEqual({ allowed_cidrs: [] })
  })
})
