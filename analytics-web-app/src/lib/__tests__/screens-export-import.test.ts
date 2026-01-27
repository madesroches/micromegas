import { buildScreensExport, parseScreensImportFile, generateUniqueName, Screen, ScreenTypeName } from '../screens-api'

function makeScreen(overrides: Partial<Screen> = {}): Screen {
  return {
    name: 'test-screen',
    screen_type: 'notebook' as ScreenTypeName,
    config: { timeRangeFrom: 'now-1h', timeRangeTo: 'now' },
    created_by: 'user1',
    updated_by: 'user2',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-02T00:00:00Z',
    ...overrides,
  }
}

describe('buildScreensExport', () => {
  it('wraps screens in export envelope with version and timestamp', () => {
    const screens = [makeScreen()]
    const json = buildScreensExport(screens)
    const parsed = JSON.parse(json)

    expect(parsed.version).toBe(1)
    expect(parsed.exported_at).toBeDefined()
    expect(parsed.screens).toHaveLength(1)
  })

  it('strips created_by, updated_by, created_at, updated_at fields', () => {
    const screens = [makeScreen()]
    const json = buildScreensExport(screens)
    const parsed = JSON.parse(json)
    const exported = parsed.screens[0]

    expect(exported.name).toBe('test-screen')
    expect(exported.screen_type).toBe('notebook')
    expect(exported.config).toEqual({ timeRangeFrom: 'now-1h', timeRangeTo: 'now' })
    expect(exported.created_by).toBeUndefined()
    expect(exported.updated_by).toBeUndefined()
    expect(exported.created_at).toBeUndefined()
    expect(exported.updated_at).toBeUndefined()
  })

  it('handles multiple screens', () => {
    const screens = [
      makeScreen({ name: 'a' }),
      makeScreen({ name: 'b', screen_type: 'log' as ScreenTypeName }),
    ]
    const json = buildScreensExport(screens)
    const parsed = JSON.parse(json)

    expect(parsed.screens).toHaveLength(2)
    expect(parsed.screens[0].name).toBe('a')
    expect(parsed.screens[1].name).toBe('b')
    expect(parsed.screens[1].screen_type).toBe('log')
  })

  it('handles empty array', () => {
    const json = buildScreensExport([])
    const parsed = JSON.parse(json)

    expect(parsed.screens).toHaveLength(0)
  })
})

describe('parseScreensImportFile', () => {
  it('parses a valid export file', () => {
    const input = JSON.stringify({
      version: 1,
      exported_at: '2026-01-01T00:00:00Z',
      screens: [
        { name: 'test', screen_type: 'notebook', config: {} },
      ],
    })

    const result = parseScreensImportFile(input)
    expect(result.version).toBe(1)
    expect(result.screens).toHaveLength(1)
    expect(result.screens[0].name).toBe('test')
  })

  it('throws on invalid JSON', () => {
    expect(() => parseScreensImportFile('not json')).toThrow('Invalid JSON file')
  })

  it('throws when not an object', () => {
    expect(() => parseScreensImportFile('"hello"')).toThrow('expected a JSON object')
  })

  it('throws when missing version', () => {
    expect(() =>
      parseScreensImportFile(JSON.stringify({ screens: [] }))
    ).toThrow('missing "version"')
  })

  it('throws when missing screens array', () => {
    expect(() =>
      parseScreensImportFile(JSON.stringify({ version: 1 }))
    ).toThrow('missing "screens" array')
  })

  it('throws when a screen is missing required fields', () => {
    const input = JSON.stringify({
      version: 1,
      screens: [{ name: 'test' }],
    })
    expect(() => parseScreensImportFile(input)).toThrow('missing required fields')
  })

  it('throws when config is null', () => {
    const input = JSON.stringify({
      version: 1,
      screens: [{ name: 'test', screen_type: 'notebook', config: null }],
    })
    expect(() => parseScreensImportFile(input)).toThrow('missing required fields')
  })

  it('roundtrips with buildScreensExport', () => {
    const screens = [
      makeScreen({ name: 'alpha' }),
      makeScreen({ name: 'beta', screen_type: 'metrics' as ScreenTypeName }),
    ]
    const json = buildScreensExport(screens)
    const parsed = parseScreensImportFile(json)

    expect(parsed.version).toBe(1)
    expect(parsed.screens).toHaveLength(2)
    expect(parsed.screens[0].name).toBe('alpha')
    expect(parsed.screens[1].name).toBe('beta')
  })
})

describe('generateUniqueName', () => {
  it('returns name-imported when no conflict', () => {
    const existing = new Set<string>()
    expect(generateUniqueName('my-screen', existing)).toBe('my-screen-imported')
  })

  it('returns name-imported-2 when name-imported already exists', () => {
    const existing = new Set(['my-screen-imported'])
    expect(generateUniqueName('my-screen', existing)).toBe('my-screen-imported-2')
  })

  it('increments counter until a free name is found', () => {
    const existing = new Set([
      'my-screen-imported',
      'my-screen-imported-2',
      'my-screen-imported-3',
    ])
    expect(generateUniqueName('my-screen', existing)).toBe('my-screen-imported-4')
  })

  it('does not collide with unrelated names', () => {
    const existing = new Set(['other-screen-imported'])
    expect(generateUniqueName('my-screen', existing)).toBe('my-screen-imported')
  })
})
