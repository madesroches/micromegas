import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import type { ScreenRendererProps } from '@/lib/screen-renderers/init'
import ScreenPage from '../ScreenPage'
import { getScreen } from '@/lib/screens-api'
import { AdminModeProvider, useAdminMode } from '@/lib/admin-mode'

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

const { refresh } = vi.hoisted(() => ({ refresh: vi.fn() }))
vi.mock('@/hooks/useRefreshInterval', () => ({
  useRefreshInterval: (...args: unknown[]) => refresh(...args),
}))

vi.mock('@/lib/screen-renderers/init', () => ({
  SCREEN_RENDERERS: {
    notebook: (props: ScreenRendererProps) => (
      <div>
        <div data-testid="renderer-admin">{String(props.adminMode)}</div>
        <button onClick={() => props.onConfigChange({ refreshIntervalMs: 30000, marker: 'edited' })}>
          edit
        </button>
      </div>
    ),
  },
}))

function Toggle() {
  const mode = useAdminMode()
  return (
    <button onClick={() => mode.setActive?.(!mode.active)}>
      {`toggle:${mode.kind}:${mode.active}`}
    </button>
  )
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/screen/foo']}>
      <AdminModeProvider pathname="/screen/foo">
        <Toggle />
        <ScreenPage />
      </AdminModeProvider>
    </MemoryRouter>
  )
}

const SAVED = { marker: 'saved', refreshIntervalMs: 30000, timeRangeFrom: 'now-1h', timeRangeTo: 'now' }

describe('ScreenPage admin mode', () => {
  beforeEach(() => {
    refresh.mockClear()
    vi.mocked(getScreen).mockResolvedValue({
      name: 'foo',
      screen_type: 'notebook',
      config: SAVED,
      created_at: '',
      updated_at: '',
      folder_path: '',
    } as never)
  })

  it('is off by default and registers a toggle', async () => {
    renderPage()
    await screen.findByTestId('renderer-admin')
    expect(screen.getByTestId('renderer-admin')).toHaveTextContent('false')
    expect(await screen.findByText('toggle:toggle:false')).toBeInTheDocument()
  })

  it('pauses auto-refresh without touching the stored interval, and restores it', async () => {
    renderPage()
    await screen.findByTestId('renderer-admin')
    expect(refresh.mock.calls.at(-1)?.[0]).toBe(30000)

    fireEvent.click(await screen.findByText('toggle:toggle:false'))
    await waitFor(() => expect(screen.getByTestId('renderer-admin')).toHaveTextContent('true'))
    expect(refresh.mock.calls.at(-1)?.[0]).toBe(0)

    fireEvent.click(await screen.findByText('toggle:toggle:true'))
    await waitFor(() => expect(screen.getByTestId('renderer-admin')).toHaveTextContent('false'))
    expect(refresh.mock.calls.at(-1)?.[0]).toBe(30000)
  })

  it('resets when the draft is discarded', async () => {
    renderPage()
    await screen.findByTestId('renderer-admin')
    fireEvent.click(await screen.findByText('toggle:toggle:false'))
    await waitFor(() => expect(screen.getByTestId('renderer-admin')).toHaveTextContent('true'))

    fireEvent.click(screen.getByText('edit'))
    fireEvent.click(screen.getByRole('button', { name: /Discard/ }))
    fireEvent.click(screen.getAllByRole('button', { name: 'Discard' }).at(-1)!)
    await waitFor(() => expect(screen.getByTestId('renderer-admin')).toHaveTextContent('false'))
    expect(screen.getByText('toggle:toggle:false')).toBeInTheDocument()
  })
})
