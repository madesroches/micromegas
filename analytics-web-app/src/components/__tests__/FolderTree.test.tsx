import { fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter, useLocation, useNavigate } from 'react-router'
import { buildFolderTree, ancestorPaths, normalizeFolderSegment, parentOf, FolderTree } from '../FolderTree'
import { FolderInfo } from '@/lib/folders-api'
import { Screen } from '@/lib/screens-api'

// The global test-setup.ts mock stubs `useLocation` to a constant value, which
// would hide real navigation. This file needs the real implementation so the
// rendering tests below can observe whether a click actually changed the
// router's location (e.g. to confirm a Ctrl/Cmd-click did not navigate).
vi.mock('react-router', async (importOriginal) => {
  const actual = await importOriginal<typeof import('react-router')>()
  return { ...actual }
})

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

function makeScreen(name: string, folderPath = ''): Screen {
  return {
    name,
    screen_type: 'table',
    config: {},
    created_at: '2026-01-01T00:00:00.000Z',
    updated_at: '2026-01-01T00:00:00.000Z',
    folder_path: folderPath,
  }
}

function LocationDisplay() {
  const location = useLocation()
  return <div data-testid="location">{location.pathname}{location.search}</div>
}

// Exposes a button that steps one entry back in history. Used to distinguish
// "navigated exactly once" from "navigated twice to the same URL": a single
// duplicate push is invisible in the final pathname (it's the same URL) but
// leaves an extra history entry, so going back one step lands back on the
// *previous* pushed entry instead of the original starting location.
function GoBackButton() {
  const navigate = useNavigate()
  return (
    <button data-testid="go-back" onClick={() => navigate(-1)}>
      back
    </button>
  )
}

function renderTree(overrides: {
  onSelectFolder?: (path: string, e: React.MouseEvent) => void
  onSelectScreen?: (name: string, e: React.MouseEvent) => void
  folders?: FolderInfo[]
  expandedPaths?: Set<string>
  onToggleExpand?: (path: string) => void
} = {}) {
  const folders: FolderInfo[] = overrides.folders ?? [{ path: 'team', screen_count: 0, subfolder_count: 0 }]
  const screens: Screen[] = [makeScreen('dashboard')]
  const onSelectFolder = overrides.onSelectFolder ?? vi.fn()
  const onSelectScreen = overrides.onSelectScreen ?? vi.fn()
  const onToggleExpand = overrides.onToggleExpand ?? vi.fn()

  render(
    <MemoryRouter initialEntries={['/screens']}>
      <FolderTree
        folders={folders}
        screens={screens}
        selectedFolder=""
        onSelectFolder={onSelectFolder}
        folderHref={(p) => `/screens?folder=${p}`}
        folderNavReplace={false}
        onSelectScreen={onSelectScreen}
        screenHref={(n) => `/screen/${n}`}
        expandedPaths={overrides.expandedPaths ?? new Set(['team'])}
        onToggleExpand={onToggleExpand}
        onDropScreen={vi.fn()}
        onCreateFolder={vi.fn()}
        onRenameFolder={vi.fn()}
        onDeleteFolder={vi.fn()}
      />
      <LocationDisplay />
      <GoBackButton />
    </MemoryRouter>
  )

  return { onSelectFolder, onSelectScreen, onToggleExpand }
}

describe('FolderTree rendering', () => {
  it('renders the folder and screen rows as real links with the expected href', () => {
    renderTree()
    expect(screen.getByRole('link', { name: /team/ })).toHaveAttribute('href', '/screens?folder=team')
    expect(screen.getByRole('link', { name: /dashboard/ })).toHaveAttribute('href', '/screen/dashboard')
  })

  it('still calls onSelectFolder/onSelectScreen when a row is clicked', () => {
    const { onSelectFolder, onSelectScreen } = renderTree()
    fireEvent.click(screen.getByRole('link', { name: /team/ }))
    expect(onSelectFolder).toHaveBeenCalledWith('team', expect.anything())
    fireEvent.click(screen.getByRole('link', { name: /dashboard/ }))
    expect(onSelectScreen).toHaveBeenCalledWith('dashboard', expect.anything())
  })

  it('keeps the new-subfolder and folder-actions buttons outside the folder link', () => {
    renderTree()
    const folderLink = screen.getByRole('link', { name: /team/ })
    for (const button of screen.getAllByRole('button')) {
      expect(folderLink.contains(button)).toBe(false)
    }
  })

  it('keeps the rename input outside the folder link once renaming starts', () => {
    renderTree()
    fireEvent.click(screen.getByTitle('Folder actions'))
    fireEvent.click(screen.getByText('Rename'))
    const input = screen.getByDisplayValue('team')
    const folderLink = document.querySelector('a[href="/screens?folder=team"]')
    expect(folderLink).not.toBeNull()
    expect(folderLink?.contains(input)).toBe(false)
  })

  // jsdom doesn't special-case modifier-key clicks the way real browsers do
  // (open a new tab, leave the current tab alone): it still attempts the
  // anchor's native navigation and logs "Not implemented: navigation" noise
  // when it can't perform it, even though the assertions below already show
  // the current tab's location never changes. Temporarily preventDefault()
  // the click at the document level to suppress that native-navigation
  // attempt without touching what the test actually verifies.
  function withNativeNavSuppressed(fn: () => void) {
    const suppress = (e: Event) => e.preventDefault()
    document.addEventListener('click', suppress)
    try {
      fn()
    } finally {
      document.removeEventListener('click', suppress)
    }
  }

  it('does not navigate the current tab on a Ctrl-click, but still runs the side effect', () => {
    const { onSelectFolder } = renderTree()
    const before = screen.getByTestId('location').textContent
    withNativeNavSuppressed(() => {
      fireEvent.click(screen.getByRole('link', { name: /team/ }), { ctrlKey: true })
    })
    expect(onSelectFolder).toHaveBeenCalledWith('team', expect.anything())
    expect(screen.getByTestId('location').textContent).toBe(before)
  })

  it('does not navigate the current tab on a meta (Cmd) click', () => {
    const { onSelectScreen } = renderTree()
    const before = screen.getByTestId('location').textContent
    withNativeNavSuppressed(() => {
      fireEvent.click(screen.getByRole('link', { name: /dashboard/ }), { metaKey: true })
    })
    expect(onSelectScreen).toHaveBeenCalledWith('dashboard', expect.anything())
    expect(screen.getByTestId('location').textContent).toBe(before)
  })

  it('navigates exactly once (no duplicate history entry) on a plain click of a screen', () => {
    renderTree()
    fireEvent.click(screen.getByRole('link', { name: /dashboard/ }))
    expect(screen.getByTestId('location').textContent).toBe('/screen/dashboard')

    // If onSelectScreen also called navigate() to the same URL, this would be
    // invisible above (same pathname either way) but would leave an extra
    // history entry. Going back one step should return to the original
    // location; a duplicate push would leave us stuck on /screen/dashboard.
    fireEvent.click(screen.getByTestId('go-back'))
    expect(screen.getByTestId('location').textContent).toBe('/screens')
  })

  it('navigates exactly once (no duplicate history entry) on a plain click of a folder', () => {
    renderTree()
    fireEvent.click(screen.getByRole('link', { name: /team/ }))
    expect(screen.getByTestId('location').textContent).toBe('/screens?folder=team')

    fireEvent.click(screen.getByTestId('go-back'))
    expect(screen.getByTestId('location').textContent).toBe('/screens')
  })

  it('toggles expand for the right folder on a chevron click without navigating (preventDefault guard)', () => {
    const { onToggleExpand } = renderTree({
      folders: [
        { path: 'team', screen_count: 0, subfolder_count: 1 },
        { path: 'team/sub', screen_count: 0, subfolder_count: 0 },
      ],
      expandedPaths: new Set(['team']),
    })
    const before = screen.getByTestId('location').textContent

    // The chevron is aria-hidden (a decorative lucide icon), so it isn't
    // reachable via getByRole; select it by its lucide-generated class name.
    const chevron = document.querySelector('.lucide-chevron-right')
    expect(chevron).not.toBeNull()
    fireEvent.click(chevron!)

    expect(onToggleExpand).toHaveBeenCalledWith('team')
    // Guards against a regression that drops the chevron's preventDefault():
    // without it, this click would fall through to the anchor's native
    // navigation and change the current tab's location.
    expect(screen.getByTestId('location').textContent).toBe(before)
  })
})
