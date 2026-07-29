import { listFolders, createFolder, moveFolder, deleteFolder } from '../folders-api'
import { ScreenApiError } from '../screens-api'

describe('folders-api', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('listFolders fetches GET /api/folders', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () =>
        Promise.resolve([
          { path: 'team', screen_count: 2, subfolder_count: 1 },
          { path: 'team/dashboards', screen_count: 2, subfolder_count: 0 },
        ]),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    const folders = await listFolders()

    expect(fetchMock).toHaveBeenCalledWith('/api/folders', expect.objectContaining({ credentials: 'include' }))
    expect(folders).toHaveLength(2)
    expect(folders[0].path).toBe('team')
  })

  it('createFolder POSTs {path}', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve({ path: 'team/new' }),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await createFolder('team/new')

    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/folders')
    expect(init.method).toBe('POST')
    expect(JSON.parse(init.body as string)).toEqual({ path: 'team/new' })
  })

  it('moveFolder PUTs {path, new_path}', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve({ path: 'squad' }),
    } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await moveFolder('team', 'squad')

    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/folders')
    expect(init.method).toBe('PUT')
    expect(JSON.parse(init.body as string)).toEqual({ path: 'team', new_path: 'squad' })
  })

  it('deleteFolder DELETEs with the path as a query param', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await deleteFolder('team/archive')

    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/folders?path=team%2Farchive')
    expect(init.method).toBe('DELETE')
  })

  it('deleteFolder throws a ScreenApiError with the backend error code on failure', async () => {
    global.fetch = vi.fn().mockResolvedValue({
      ok: false,
      status: 400,
      json: () => Promise.resolve({ code: 'FOLDER_NOT_EMPTY', message: 'not empty' }),
    } as unknown as Response) as unknown as typeof fetch

    await expect(deleteFolder('team')).rejects.toMatchObject({
      code: 'FOLDER_NOT_EMPTY',
    })
    await expect(deleteFolder('team')).rejects.toBeInstanceOf(ScreenApiError)
  })
})
