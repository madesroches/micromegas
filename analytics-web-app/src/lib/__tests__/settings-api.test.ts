/**
 * `settings-api.ts`: path encoding, request bodies, and error surfacing. Mirrors
 * `groups-api.test.ts`'s shape.
 */
import { SettingsError, fetchSettings, resetSetting, setSetting } from '../settings-api'

function mockFetch(response: { ok: boolean; status: number; json: () => unknown }) {
  const fetchMock = vi.fn().mockResolvedValue({
    ok: response.ok,
    status: response.status,
    json: () => Promise.resolve(response.json()),
  } as unknown as Response)
  global.fetch = fetchMock as unknown as typeof fetch
  return fetchMock
}

const entry = {
  key: 'self_service_mint',
  kind: 'bool',
  env_var: 'MICROMEGAS_SELF_SERVICE_MINT',
  default: false,
  description: 'x',
  value: true,
  updated_at: '2026-10-21T09:14:00Z',
  updated_by: 'admin@example.com',
}

describe('settings-api', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('fetchSettings decodes a JSON array', async () => {
    const fetchMock = mockFetch({ ok: true, status: 200, json: () => [entry] })
    expect(await fetchSettings()).toEqual([entry])
    expect(String(fetchMock.mock.calls[0][0])).toMatch(/\/api\/settings$/)
  })

  it('setSetting PUTs {value} to the encoded key', async () => {
    const fetchMock = mockFetch({ ok: true, status: 200, json: () => entry })
    await setSetting('public_view_sets', ['a', 'b'])
    const [url, init] = fetchMock.mock.calls[0]
    expect(String(url)).toMatch(/\/api\/settings\/public_view_sets$/)
    expect(init.method).toBe('PUT')
    expect(JSON.parse(init.body as string)).toEqual({ value: ['a', 'b'] })
  })

  it('setSetting surfaces the server message and status', async () => {
    mockFetch({
      ok: false,
      status: 400,
      json: () => ({ code: 'BAD_REQUEST', message: 'expected a JSON boolean' }),
    })
    const err = await setSetting('self_service_mint', 1 as unknown as boolean).catch((e) => e)
    expect(err).toBeInstanceOf(SettingsError)
    expect(err.status).toBe(400)
    expect(err.message).toBe('expected a JSON boolean')
  })

  it('resetSetting accepts 204 and DELETEs the encoded key', async () => {
    const fetchMock = mockFetch({ ok: true, status: 204, json: () => undefined })
    await resetSetting('self_service_mint')
    const [url, init] = fetchMock.mock.calls[0]
    expect(String(url)).toMatch(/\/api\/settings\/self_service_mint$/)
    expect(init.method).toBe('DELETE')
  })

  it('resetSetting throws on a 404', async () => {
    mockFetch({ ok: false, status: 404, json: () => ({ code: 'NOT_FOUND', message: 'setting not found' }) })
    await expect(resetSetting('self_service_mint')).rejects.toMatchObject({ status: 404 })
  })
})
