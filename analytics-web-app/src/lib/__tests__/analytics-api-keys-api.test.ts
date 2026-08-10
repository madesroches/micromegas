/**
 * `listAnalyticsApiKeys` must explicitly request the server's max limit
 * (500) rather than omitting `limit` (which falls back to the server's
 * lower `DEFAULT_LIMIT` of 100) — omitting it silently truncates the list on
 * any deployment with more than 100 lifetime keys, with no indication
 * anything is missing. It must also thread `offset` through so the page can
 * page past the first 500 lifetime keys.
 */
import { listAnalyticsApiKeys, MAX_ANALYTICS_API_KEYS_LIST_LIMIT } from '../analytics-api-keys-api'

describe('analytics-api-keys-api', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('listAnalyticsApiKeys requests the server max limit and offset 0 by default', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve([]),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await listAnalyticsApiKeys()

    const [url] = fetchMock.mock.calls[0]
    expect(url).toBe(
      `/api/analytics-api-keys?limit=${MAX_ANALYTICS_API_KEYS_LIST_LIMIT}&offset=0&include_revoked=true`
    )
    expect(MAX_ANALYTICS_API_KEYS_LIST_LIMIT).toBe(500)
  })

  it('listAnalyticsApiKeys forwards a non-zero offset', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve([]),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await listAnalyticsApiKeys(true, MAX_ANALYTICS_API_KEYS_LIST_LIMIT)

    const [url] = fetchMock.mock.calls[0]
    expect(url).toBe(
      `/api/analytics-api-keys?limit=${MAX_ANALYTICS_API_KEYS_LIST_LIMIT}&offset=${MAX_ANALYTICS_API_KEYS_LIST_LIMIT}&include_revoked=true`
    )
  })
})
