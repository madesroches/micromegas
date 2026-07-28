import { buildFolderTree, ancestorPaths, normalizeFolderSegment, parentOf } from '../FolderTree'
import { FolderInfo } from '@/lib/folders-api'

function folder(path: string, screenCount = 0, subfolderCount = 0): FolderInfo {
  return { path, screen_count: screenCount, subfolder_count: subfolderCount }
}

describe('buildFolderTree', () => {
  it('builds a nested tree from flat folder paths', () => {
    const tree = buildFolderTree([
      folder('team'),
      folder('team/dashboards'),
      folder('team/dashboards/incidents'),
      folder('archive'),
    ])

    expect(tree.children.map((c) => c.name).sort()).toEqual(['archive', 'team'])

    const team = tree.children.find((c) => c.path === 'team')
    expect(team?.children.map((c) => c.path)).toEqual(['team/dashboards'])

    const dashboards = team?.children[0]
    expect(dashboards?.children.map((c) => c.path)).toEqual(['team/dashboards/incidents'])
  })

  it('sorts children alphabetically at every level', () => {
    const tree = buildFolderTree([folder('zeta'), folder('alpha'), folder('mid')])
    expect(tree.children.map((c) => c.name)).toEqual(['alpha', 'mid', 'zeta'])
  })

  it('handles an empty folder list', () => {
    const tree = buildFolderTree([])
    expect(tree.path).toBe('')
    expect(tree.children).toEqual([])
  })

  it('deduplicates ancestors implied by multiple deep paths', () => {
    const tree = buildFolderTree([folder('a/b/c'), folder('a/b/d')])
    expect(tree.children).toHaveLength(1)
    const a = tree.children[0]
    expect(a.path).toBe('a')
    expect(a.children).toHaveLength(1)
    const b = a.children[0]
    expect(b.path).toBe('a/b')
    expect(b.children.map((c) => c.path).sort()).toEqual(['a/b/c', 'a/b/d'])
  })
})

describe('ancestorPaths', () => {
  it('returns an empty array for root', () => {
    expect(ancestorPaths('')).toEqual([])
  })

  it('returns an empty array for a top-level folder', () => {
    expect(ancestorPaths('team')).toEqual([])
  })

  it('returns every ancestor prefix, excluding the path itself', () => {
    expect(ancestorPaths('team/dashboards/incidents')).toEqual(['team', 'team/dashboards'])
  })
})

describe('normalizeFolderSegment', () => {
  it('lowercases and hyphenates spaces', () => {
    expect(normalizeFolderSegment('My Folder')).toBe('my-folder')
  })

  it('strips invalid characters', () => {
    expect(normalizeFolderSegment('team@#$dashboards')).toBe('teamdashboards')
  })

  it('collapses consecutive hyphens', () => {
    expect(normalizeFolderSegment('a--b')).toBe('a-b')
  })

  it('trims leading/trailing hyphens', () => {
    expect(normalizeFolderSegment('-team-')).toBe('team')
  })

  it('allows a purely numeric segment (unlike screen names)', () => {
    expect(normalizeFolderSegment('2025')).toBe('2025')
  })
})

describe('parentOf', () => {
  it('returns the root for a top-level folder', () => {
    expect(parentOf('team')).toBe('')
  })

  it('returns everything before the last segment', () => {
    expect(parentOf('team/dashboards/incidents')).toBe('team/dashboards')
  })
})
