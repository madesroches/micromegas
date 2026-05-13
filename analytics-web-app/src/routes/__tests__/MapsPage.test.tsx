/**
 * Light-touch tests for `MapsPage` — exercise the page's contract with
 * the catalog helper and confirm the three user actions (initial load,
 * upload, delete) hit the right URLs. Heavy mocking of pixel layout is
 * intentionally avoided; we render the page and probe the fetch calls.
 */
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import MapsPage from '../MapsPage'

// Force useAuth to report an admin user so AuthGuard renders the page.
jest.mock('@/lib/auth', () => ({
  useAuth: () => ({
    status: 'authenticated',
    user: { sub: 'admin', is_admin: true },
    error: null,
  }),
}))

// Pin basePath to a known value so the URL assertions are stable.
jest.mock('@/lib/config', () => ({
  getConfig: () => ({ basePath: '/mmlocal' }),
  appLink: (path: string) => `/mmlocal${path}`,
}))

// The page-title hook reads from window during effects; stub it out so
// jsdom doesn't warn on the document title side effect.
jest.mock('@/hooks/usePageTitle', () => ({ usePageTitle: () => undefined }))

// PageLayout pulls in a fair amount (header, sidebar). Stub it down to a
// simple pass-through wrapper — the page's contract under test is the
// catalog list, the upload button, and the delete confirmation, not the
// app chrome.
jest.mock('@/components/layout', () => ({
  PageLayout: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))

function renderPage() {
  return render(
    <MemoryRouter>
      <MapsPage />
    </MemoryRouter>
  )
}

describe('MapsPage', () => {
  afterEach(() => {
    jest.restoreAllMocks()
  })

  it('renders the empty state when the catalog is empty', async () => {
    global.fetch = jest.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve([]),
    } as unknown as Response) as unknown as typeof fetch

    renderPage()

    await waitFor(() =>
      expect(screen.getByText(/No maps uploaded yet/i)).toBeInTheDocument()
    )
  })

  it('lists existing maps from the catalog response', async () => {
    global.fetch = jest.fn().mockResolvedValue({
      ok: true,
      json: () =>
        Promise.resolve([
          { file: 'main.glb', size: 2048, last_modified: '2026-01-02T03:04:05Z' },
          { file: 'level_a.glb', size: 10240, last_modified: '2026-02-02T03:04:05Z' },
        ]),
    } as unknown as Response) as unknown as typeof fetch

    renderPage()

    await waitFor(() => expect(screen.getByText('main')).toBeInTheDocument())
    expect(screen.getByText('level_a')).toBeInTheDocument()
  })

  it('opens the delete confirm dialog and DELETEs the right URL on confirm', async () => {
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        json: () =>
          Promise.resolve([
            { file: 'main.glb', size: 1, last_modified: '2026-01-01T00:00:00Z' },
          ]),
      } as unknown as Response)
      .mockResolvedValueOnce({ ok: true, status: 204 } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve([]),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage()

    await waitFor(() => expect(screen.getByText('main')).toBeInTheDocument())

    fireEvent.click(screen.getByRole('button', { name: /Delete main\.glb/i }))
    // Confirm dialog: click the Delete confirm button (inside the dialog title "Delete Map")
    const confirmButton = await screen.findByRole('button', { name: 'Delete' })
    await act(async () => {
      fireEvent.click(confirmButton)
    })

    await waitFor(() => {
      const deleteCall = fetchMock.mock.calls.find((c) => c[1]?.method === 'DELETE')
      expect(deleteCall).toBeDefined()
      expect(deleteCall![0]).toBe('/mmlocal/api/maps/blob/main.glb')
    })
  })

  it('uploads via the file input and refreshes the catalog', async () => {
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve([]),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({ file: 'new.glb', size: 12 }),
      } as unknown as Response)
      .mockResolvedValueOnce({
        ok: true,
        json: () =>
          Promise.resolve([
            { file: 'new.glb', size: 12, last_modified: '2026-01-01T00:00:00Z' },
          ]),
      } as unknown as Response)
    global.fetch = fetchMock as unknown as typeof fetch

    renderPage()

    await waitFor(() =>
      expect(screen.getByText(/No maps uploaded yet/i)).toBeInTheDocument()
    )

    const fileInput = screen.getByLabelText(/Choose GLB file/i) as HTMLInputElement
    const file = new File([new Uint8Array([1, 2, 3])], 'new.glb', {
      type: 'model/gltf-binary',
    })
    await act(async () => {
      fireEvent.change(fileInput, { target: { files: [file] } })
    })

    await waitFor(() => {
      const putCall = fetchMock.mock.calls.find((c) => c[1]?.method === 'PUT')
      expect(putCall).toBeDefined()
      expect(putCall![0]).toBe('/mmlocal/api/maps/blob/new.glb')
      expect(putCall![1].headers).toEqual(
        expect.objectContaining({ 'Content-Type': 'model/gltf-binary' })
      )
    })

    await waitFor(() => expect(screen.getByText('new')).toBeInTheDocument())
  })
})
