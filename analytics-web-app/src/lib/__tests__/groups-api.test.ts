/**
 * `groups-api.ts`: decoding `GET /api/groups[/...]`, create/add status handling, and error
 * surfacing. Mirrors `audience-grants-api.test.ts`'s shape.
 */
import {
  GroupsError,
  addGroupMember,
  createGroup,
  deleteGroup,
  fetchGroupMembers,
  fetchGroups,
  removeGroupMember,
} from '../groups-api'

function mockFetch(response: { ok: boolean; status: number; json: () => unknown }) {
  const fetchMock = vi.fn().mockResolvedValue({
    ok: response.ok,
    status: response.status,
    json: () => Promise.resolve(response.json()),
  } as unknown as Response)
  global.fetch = fetchMock as unknown as typeof fetch
  return fetchMock
}

describe('groups-api', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('fetchGroups decodes a JSON array', async () => {
    mockFetch({
      ok: true,
      status: 200,
      json: () => [
        {
          name: 'admins',
          description: 'Deployment administrators',
          member_count: 1,
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'default',
        },
      ],
    })

    const groups = await fetchGroups()
    expect(groups).toEqual([
      {
        name: 'admins',
        description: 'Deployment administrators',
        member_count: 1,
        created_at: '2026-08-14T00:00:00Z',
        created_by: 'default',
      },
    ])
  })

  it('fetchGroups surfaces a non-2xx as GroupsError', async () => {
    mockFetch({
      ok: false,
      status: 503,
      json: () => ({ code: 'NOT_CONFIGURED', message: 'group store not configured' }),
    })

    await expect(fetchGroups()).rejects.toMatchObject({
      name: 'GroupsError',
      code: 'NOT_CONFIGURED',
      status: 503,
    })
  })

  it('createGroup posts name/description and decodes the created row', async () => {
    const fetchMock = mockFetch({
      ok: true,
      status: 201,
      json: () => ({
        name: 'eng',
        description: null,
        member_count: 0,
        created_at: '2026-08-14T00:00:00Z',
        created_by: 'admin@example.com',
      }),
    })

    const group = await createGroup('eng')
    expect(group.name).toBe('eng')
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/groups')
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({ name: 'eng' })
  })

  it('createGroup surfaces a 409 as GroupsError', async () => {
    mockFetch({
      ok: false,
      status: 409,
      json: () => ({ code: 'CONFLICT', message: 'group "eng" already exists' }),
    })

    await expect(createGroup('eng')).rejects.toMatchObject({ code: 'CONFLICT', status: 409 })
  })

  it('deleteGroup encodes the name and resolves on 204', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await expect(deleteGroup('eng team')).resolves.toBeUndefined()

    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/groups/eng%20team')
    expect((init as RequestInit).method).toBe('DELETE')
  })

  it('deleteGroup surfaces a 409 (still referenced) as GroupsError', async () => {
    mockFetch({
      ok: false,
      status: 409,
      json: () => ({ code: 'CONFLICT', message: 'group "eng" is still referenced' }),
    })

    await expect(deleteGroup('eng')).rejects.toMatchObject({ code: 'CONFLICT', status: 409 })
  })

  it('fetchGroupMembers decodes a JSON array', async () => {
    mockFetch({
      ok: true,
      status: 200,
      json: () => [
        {
          group_name: 'admins',
          member: '*',
          created_at: '2026-08-14T00:00:00Z',
          created_by: 'default',
        },
      ],
    })

    const members = await fetchGroupMembers('admins')
    expect(members[0].member).toBe('*')
  })

  it('addGroupMember reports created: true on 201', async () => {
    const fetchMock = mockFetch({
      ok: true,
      status: 201,
      json: () => ({
        group_name: 'eng',
        member: 'user:alice@example.com',
        created_at: '2026-08-14T00:00:00Z',
        created_by: 'admin@example.com',
      }),
    })

    const { created, member } = await addGroupMember('eng', 'user:alice@example.com')
    expect(created).toBe(true)
    expect(member.member).toBe('user:alice@example.com')
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/groups/eng/members')
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      member: 'user:alice@example.com',
    })
  })

  it('addGroupMember reports created: false on 200 (already existed)', async () => {
    mockFetch({
      ok: true,
      status: 200,
      json: () => ({
        group_name: 'eng',
        member: 'user:alice@example.com',
        created_at: '2026-08-14T00:00:00Z',
        created_by: 'admin@example.com',
      }),
    })

    const { created } = await addGroupMember('eng', 'user:alice@example.com')
    expect(created).toBe(false)
  })

  it('addGroupMember surfaces a 404 (nested group does not exist) as GroupsError', async () => {
    mockFetch({
      ok: false,
      status: 404,
      json: () => ({ code: 'NOT_FOUND', message: 'group not found' }),
    })

    await expect(addGroupMember('eng', 'group:ghost')).rejects.toMatchObject({
      code: 'NOT_FOUND',
      status: 404,
    })
  })

  it('addGroupMember surfaces a 409 (cycle) as GroupsError', async () => {
    mockFetch({
      ok: false,
      status: 409,
      json: () => ({ code: 'CONFLICT', message: 'would create a cycle' }),
    })

    await expect(addGroupMember('a', 'group:b')).rejects.toMatchObject({
      code: 'CONFLICT',
      status: 409,
    })
  })

  it('removeGroupMember encodes the member query param and resolves on 204', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    await expect(removeGroupMember('eng', 'group:eng/leads')).resolves.toBeUndefined()

    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/groups/eng/members?member=group%3Aeng%2Fleads')
    expect((init as RequestInit).method).toBe('DELETE')
  })

  it('removeGroupMember surfaces a 409 (last admins row) as GroupsError', async () => {
    mockFetch({
      ok: false,
      status: 409,
      json: () => ({
        code: 'CONFLICT',
        message: 'removing the last member of admins would leave admin reachable only through',
      }),
    })

    await expect(removeGroupMember('admins', '*')).rejects.toBeInstanceOf(GroupsError)
  })
})
