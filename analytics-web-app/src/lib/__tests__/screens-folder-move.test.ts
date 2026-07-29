import { updateScreen, createScreen } from '../screens-api'

describe('updateScreen — folder moves never rename', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('a folder-only move sends only folder_path in the body, never a name field', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () =>
        Promise.resolve({
          name: 'my-screen',
          screen_type: 'notebook',
          config: {},
          created_at: '2026-01-01T00:00:00Z',
          updated_at: '2026-01-01T00:00:00Z',
          folder_path: 'team/dashboards',
        }),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await updateScreen('my-screen', { folder_path: 'team/dashboards' })

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/screens/my-screen')
    expect(init.method).toBe('PUT')
    const body = JSON.parse(init.body as string)
    expect(body).toEqual({ folder_path: 'team/dashboards' })
    expect(body.name).toBeUndefined()
    expect(body.config).toBeUndefined()
  })

  it('identity (the URL path segment) stays the screen name regardless of destination', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve({}),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await updateScreen('unmoved-name', { folder_path: 'a/b/c' })

    const [url] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/screens/unmoved-name')
  })
})

describe('createScreen — folder_path passthrough', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('includes folder_path in the create request body', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve({}),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await createScreen({
      name: 'new-screen',
      screen_type: 'notebook',
      config: {},
      folder_path: 'team',
    })

    const [, init] = fetchMock.mock.calls[0]
    const body = JSON.parse(init.body as string)
    expect(body.folder_path).toBe('team')
  })
})
