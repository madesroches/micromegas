/**
 * `listIngestionApiKeys` must send the caller's `limit` explicitly rather than
 * omitting it (which falls back to the server's lower `DEFAULT_LIMIT` of
 * 100) — omitting it silently truncates the list on any deployment with more
 * than 100 lifetime keys, with no indication anything is missing. The page
 * passes `MAX_INGESTION_API_KEYS_LIST_LIMIT` (the server's max), so that's what
 * goes on the wire. `offset` must be threaded through too, so the page can
 * page past the first 500 lifetime keys.
 */
import { listIngestionApiKeys, MAX_INGESTION_API_KEYS_LIST_LIMIT } from '../ingestion-api-keys-api'

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
})
