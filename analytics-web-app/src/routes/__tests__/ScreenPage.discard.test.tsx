import { useEffect, useState } from 'react'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { MemoryRouter, useLocation, useSearchParams } from 'react-router'
import type { ScreenRendererProps } from '@/lib/screen-renderers/init'
import ScreenPage from '../ScreenPage'
import { getScreen, updateScreen } from '@/lib/screens-api'

vi.mock('react-router', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-router')>()),
  useParams: () => ({ name: 'foo' }),
}))

vi.mock('@/lib/auth', () => ({
  useAuth: () => ({ status: 'authenticated', user: { sub: 'admin', is_admin: true }, error: null }),
}))
vi.mock('@/hooks/usePageTitle', () => ({ usePageTitle: () => undefined }))
vi.mock('@/hooks/useDefaultDataSource', () => ({
  useDefaultDataSource: () => ({ name: 'ds', error: null }),
}))
vi.mock('@/components/layout', () => ({
  PageLayout: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))

vi.mock('@/lib/screens-api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/screens-api')>()),
  getScreen: vi.fn(),
  getScreenTypes: vi.fn(async () => [
    { name: 'notebook', display_name: 'Notebook', icon: 'file-text', description: '' },
  ]),
  getDefaultConfig: vi.fn(),
  updateScreen: vi.fn(),
}))

const { mounts } = vi.hoisted(() => ({ mounts: { count: 0, seeds: [] as string[] } }))

vi.mock('@/lib/screen-renderers/init', () => {
  // Seeds local state from the URL and writes it back on mount, like renderers that own URL state.
  function StubRenderer(props: ScreenRendererProps) {
    const [params, setParams] = useSearchParams()
    const [seed] = useState(() => params.get('var') ?? '')
    useEffect(() => {
      mounts.count++
      mounts.seeds.push(seed)
    }, [seed])
    // Re-runs whenever setSearchParams changes identity (i.e. on every URL change), like the
    // real renderers' URL-sync effects.
    useEffect(() => {
      if (seed) {
        const next = new URLSearchParams(params)
        next.set('var', seed)
        setParams(next, { replace: true })
      }
    }, [seed, params, setParams])
    return (
      <div>
        <div data-testid="seed">{seed}</div>
        <div data-testid="cfg">{String(props.config.marker)}</div>
        <div data-testid="range">{`${props.rawTimeRange.from}|${props.rawTimeRange.to}`}</div>
        <button onClick={() => props.onConfigChange({ marker: 'edited' })}>edit</button>
      </div>
    )
  }
  return { SCREEN_RENDERERS: { notebook: StubRenderer } }
})

const SAVED = { marker: 'saved', timeRangeFrom: 'now-1h', timeRangeTo: 'now' }

function makeScreen(extra: Record<string, unknown> = {}) {
  return {
    name: 'foo',
    screen_type: 'notebook',
    config: SAVED,
    created_at: '',
    updated_at: '',
    folder_path: '',
    ...extra,
  }
}

function LocationProbe() {
  const location = useLocation()
  return <div data-testid="search">{location.search}</div>
}

function renderPage(url = '/screen/foo') {
  return render(
    <MemoryRouter initialEntries={[url]}>
      <ScreenPage />
      <LocationProbe />
    </MemoryRouter>
  )
}

async function loaded() {
  await screen.findByTestId('cfg')
}

describe('ScreenPage discard', () => {
  beforeEach(() => {
    mounts.count = 0
    mounts.seeds = []
    vi.mocked(getScreen).mockResolvedValue(makeScreen() as never)
  })

  it('hides Discard when there are no unsaved changes', async () => {
    renderPage()
    await loaded()
    expect(screen.queryByRole('button', { name: /Discard/ })).not.toBeInTheDocument()
  })

  it('shows Discard after an edit', async () => {
    renderPage()
    await loaded()
    fireEvent.click(screen.getByText('edit'))
    expect(screen.getByRole('button', { name: /Discard/ })).toBeEnabled()
  })

  it('disables Discard while saving', async () => {
    vi.mocked(updateScreen).mockReturnValue(new Promise(() => {}))
    renderPage()
    await loaded()
    fireEvent.click(screen.getByText('edit'))
    fireEvent.click(screen.getByRole('button', { name: /^Save$/ }))
    await waitFor(() => expect(screen.getByRole('button', { name: /Discard/ })).toBeDisabled())
  })

  it('cancel leaves config untouched', async () => {
    renderPage()
    await loaded()
    fireEvent.click(screen.getByText('edit'))
    fireEvent.click(screen.getByRole('button', { name: /Discard/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(screen.getByTestId('cfg')).toHaveTextContent('edited')
    expect(screen.getByText('(unsaved changes)')).toBeInTheDocument()
  })

  it('confirm restores saved config, clears the query and time range', async () => {
    renderPage('/screen/foo?from=now-2h&to=now-1h&var=x')
    await loaded()
    fireEvent.click(screen.getByText('edit'))
    expect(screen.getByTestId('range')).toHaveTextContent('now-2h|now-1h')
    fireEvent.click(screen.getByRole('button', { name: /Discard/ }))
    fireEvent.click(screen.getAllByRole('button', { name: 'Discard' }).pop()!)
    await waitFor(() => expect(screen.getByTestId('cfg')).toHaveTextContent('saved'))
    expect(screen.getByTestId('search').textContent).toBe('')
    expect(screen.getByTestId('range')).toHaveTextContent('now-1h|now')
    expect(screen.queryByText('(unsaved changes)')).not.toBeInTheDocument()
  })

  it('remounts the renderer with empty params and nothing rewrites the URL', async () => {
    renderPage('/screen/foo?var=x')
    await loaded()
    expect(mounts.seeds).toEqual(['x'])
    fireEvent.click(screen.getByText('edit'))
    fireEvent.click(screen.getByRole('button', { name: /Discard/ }))
    fireEvent.click(screen.getAllByRole('button', { name: 'Discard' }).pop()!)
    await waitFor(() => expect(mounts.seeds).toEqual(['x', '']))
    expect(screen.getByTestId('seed')).toHaveTextContent('')
    expect(screen.getByTestId('search').textContent).toBe('')
  })
})

describe('ScreenPage managed-by indicator', () => {
  beforeEach(() => {
    vi.mocked(getScreen).mockResolvedValue(makeScreen() as never)
  })

  const indicator = () => screen.queryByRole('button', { name: /managed by source control/i })

  it('is hidden without unsaved changes', async () => {
    vi.mocked(getScreen).mockResolvedValue(makeScreen({ managed_by: 'https://example.com/s' }) as never)
    renderPage()
    await loaded()
    expect(indicator()).not.toBeInTheDocument()
  })

  it('is hidden when the screen is not managed', async () => {
    renderPage()
    await loaded()
    fireEvent.click(screen.getByText('edit'))
    expect(indicator()).not.toBeInTheDocument()
  })

  it('precedes Save and replaces the banner', async () => {
    vi.mocked(getScreen).mockResolvedValue(makeScreen({ managed_by: 'https://example.com/s' }) as never)
    renderPage()
    await loaded()
    fireEvent.click(screen.getByText('edit'))
    const save = screen.getByRole('button', { name: /^Save$/ })
    expect(
      indicator()!.compareDocumentPosition(save) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    expect(screen.queryByText(/Edits made here may be overwritten/)).not.toBeInTheDocument()
  })
})
