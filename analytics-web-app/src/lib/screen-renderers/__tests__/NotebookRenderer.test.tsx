/**
 * Tests for NotebookRenderer component
 */
import { render, screen, fireEvent, waitFor, within, act } from '@testing-library/react'
import React from 'react'

// Mock streamQuery to prevent actual API calls
const { mockStreamQuery } = vi.hoisted(() => ({ mockStreamQuery: vi.fn() }))
vi.mock('@/lib/arrow-stream', () => ({
  streamQuery: (...args: unknown[]) => mockStreamQuery(...args),
}))

// Mock Apache Arrow
vi.mock('apache-arrow', () => ({
  Table: class MockTable {
    numRows = 0
    numCols = 0
    constructor(public batches: unknown[] = []) {}
  },
}))

// Mock lucide-react icons
vi.mock('lucide-react', () => ({
  Plus: () => <span data-testid="plus-icon">+</span>,
  X: () => <span data-testid="x-icon">×</span>,
  ChevronDown: () => <span data-testid="chevron-down">▼</span>,
  ChevronRight: () => <span data-testid="chevron-right">▶</span>,
  Play: () => <span data-testid="play">▶</span>,
  RotateCcw: () => <span data-testid="rotate">↻</span>,
  MoreVertical: () => <span data-testid="more">⋮</span>,
  Trash2: () => <span data-testid="trash">🗑</span>,
  GripVertical: () => <span data-testid="grip">⠿</span>,
  Zap: () => <span data-testid="zap">⚡</span>,
  Copy: () => <span data-testid="copy">📋</span>,
  Download: () => <span data-testid="download">⬇</span>,
  Settings: () => <span data-testid="settings">⚙</span>,
  Save: () => <span data-testid="save">💾</span>,
  Database: () => <span data-testid="database">🗄</span>,
  AlertCircle: () => <span data-testid="alert-circle">⚠</span>,
  ChevronLeft: () => <span data-testid="chevron-left">◀</span>,
  ArrowLeft: () => <span data-testid="arrow-left">←</span>,
  Group: () => <span data-testid="group">⊞</span>,
  Pencil: () => <span data-testid="pencil">✏</span>,
}))

// @radix-ui/react-dropdown-menu is mocked via test.alias in vite.config.ts

// Mock @dnd-kit to simplify testing
vi.mock('@dnd-kit/core', () => ({
  DndContext: ({ children }: { children: React.ReactNode }) => <div data-testid="dnd-context">{children}</div>,
  closestCenter: vi.fn(),
  KeyboardSensor: vi.fn(),
  PointerSensor: vi.fn(),
  useSensor: vi.fn(() => ({})),
  useSensors: vi.fn(() => []),
  DragOverlay: ({ children }: { children: React.ReactNode }) => <div data-testid="drag-overlay">{children}</div>,
}))

vi.mock('@dnd-kit/sortable', () => ({
  arrayMove: (arr: unknown[], from: number, to: number) => {
    const result = [...arr] as unknown[]
    const [removed] = result.splice(from, 1)
    result.splice(to, 0, removed)
    return result
  },
  SortableContext: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="sortable-context">{children}</div>
  ),
  sortableKeyboardCoordinates: vi.fn(),
  useSortable: () => ({
    attributes: {},
    listeners: {},
    setNodeRef: vi.fn(),
    transform: null,
    transition: null,
    isDragging: false,
  }),
  verticalListSortingStrategy: vi.fn(),
}))

vi.mock('@dnd-kit/utilities', () => ({
  CSS: {
    Transform: {
      toString: () => '',
    },
  },
}))

// Mock data sources API (used by DataSourceSelector in CellEditor)
// Return a never-settling promise to avoid act() warnings from async state updates.
// These tests don't exercise data source selection.
vi.mock('@/lib/data-sources-api', () => ({
  getDataSourceList: vi.fn().mockReturnValue(new Promise(() => {})),
}))

// Mock the cell registry
vi.mock('../cell-registry', async () => {
  const { createCellRegistryMock } = await import('../__test-utils__/cell-registry-mock')
  return createCellRegistryMock({ withRenderers: true, withEditors: true })
})

// Import after mocks are set up
import { NotebookRenderer } from '../NotebookRenderer'
import { ScreenRendererProps } from '../index'
import { CellConfig } from '../notebook-utils'
import { CELL_TYPE_METADATA } from '../cell-registry'

// Helper to create default props
function createDefaultProps(overrides: Partial<ScreenRendererProps> = {}): ScreenRendererProps {
  return {
    config: { cells: [] },
    onConfigChange: vi.fn(),
    savedConfig: { cells: [] },
    timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
    rawTimeRange: { from: 'now-5m', to: 'now' },
    onTimeRangeChange: vi.fn(),
    timeRangeLabel: 'Last 1 hour',
    currentValues: {},
    onSave: vi.fn(),
    refreshTrigger: 0,
    ...overrides,
  }
}

// Helper to render NotebookRenderer and wait for async cell execution to complete
async function renderNotebook(props: ScreenRendererProps) {
  let result: ReturnType<typeof render>
  await act(async () => {
    result = render(<NotebookRenderer {...props} />)
    // Allow async cell execution to complete
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
  return result!
}

// Helper to create cell configs
function createTableCell(name: string, sql = 'SELECT 1'): CellConfig {
  return { type: 'table', name, sql, layout: { height: 'auto' } }
}

function createMarkdownCell(name: string, content = '# Notes'): CellConfig {
  return { type: 'markdown', name, content, layout: { height: 'auto' } }
}

function createVariableCell(
  name: string,
  variableType: 'text' | 'expression' | 'combobox' = 'text'
): CellConfig {
  return {
    type: 'variable',
    name,
    variableType,
    defaultValue: '',
    sql: variableType === 'combobox' ? 'SELECT value FROM options' : undefined,
    layout: { height: 'auto' },
  }
}

function createChartCell(name: string, sql = 'SELECT time, value FROM metrics'): CellConfig {
  return { type: 'chart', name, sql, layout: { height: 'auto' } }
}

describe('NotebookRenderer', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    // Default mock for successful queries - synchronously return done
    mockStreamQuery.mockImplementation(async function* () {
      yield { type: 'done' }
    })
  })

  describe('initial rendering', () => {
    it('should render empty notebook with add cell button', async () => {
      await renderNotebook(createDefaultProps())

      expect(screen.getByText('Add Cell')).toBeInTheDocument()
    })

    it('should render cells from config', async () => {
      const cells = [createTableCell('Query 1'), createTableCell('Query 2')]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      expect(screen.getByText('Query 1')).toBeInTheDocument()
      expect(screen.getByText('Query 2')).toBeInTheDocument()
    })

    it('should render different cell types', async () => {
      const cells = [
        createTableCell('MyTable'),
        createMarkdownCell('Notes'),
        createVariableCell('Filter'),
      ]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      expect(screen.getByText('MyTable')).toBeInTheDocument()
      expect(screen.getByText('Notes')).toBeInTheDocument()
      expect(screen.getByText('Filter')).toBeInTheDocument()
    })
  })

  describe('add cell modal', () => {
    it('should open add cell modal when add button is clicked', async () => {
      await renderNotebook(createDefaultProps())

      fireEvent.click(screen.getByText('Add Cell'))

      expect(screen.getByText('Table')).toBeInTheDocument()
      expect(screen.getByText('Chart')).toBeInTheDocument()
      expect(screen.getByText('Log')).toBeInTheDocument()
      expect(screen.getByText('Markdown')).toBeInTheDocument()
      expect(screen.getByText('Variable')).toBeInTheDocument()
    })

    it('should close modal when X button is clicked', async () => {
      await renderNotebook(createDefaultProps())

      fireEvent.click(screen.getByText('Add Cell'))
      expect(screen.getByRole('heading', { name: 'Add Cell' })).toBeInTheDocument()

      // Click the X button in the modal
      const modal = screen.getByRole('heading', { name: 'Add Cell' }).closest('div[class*="bg-app-panel"]')
      const closeButton = within(modal!).getByTestId('x-icon').closest('button')
      fireEvent.click(closeButton!)

      // Modal should be closed - the "Add Cell" heading should be gone
      expect(screen.queryByRole('heading', { name: 'Add Cell' })).not.toBeInTheDocument()
    })

    it('should add a new cell when type is selected', async () => {
      const onConfigChange = vi.fn()

      await renderNotebook(
        createDefaultProps({
          onConfigChange,
        })
      )

      fireEvent.click(screen.getByText('Add Cell'))

      // Find the Table button in the modal (not the badge)
      const modal = screen.getByRole('heading', { name: 'Add Cell' }).closest('div[class*="bg-app-panel"]')
      const tableButton = within(modal!).getByText('Table').closest('button')
      fireEvent.click(tableButton!)

      expect(onConfigChange).toHaveBeenCalled()

      // Modal should close after adding
      expect(screen.queryByRole('heading', { name: 'Add Cell' })).not.toBeInTheDocument()
    })

    it('should generate unique names for new cells', async () => {
      const existingCells = [createTableCell('Table')]
      const onConfigChange = vi.fn()

      await renderNotebook(
        createDefaultProps({
          config: { cells: existingCells },
          onConfigChange,
        })
      )

      fireEvent.click(screen.getByText('Add Cell'))

      const modal = screen.getByRole('heading', { name: 'Add Cell' }).closest('div[class*="bg-app-panel"]')
      const tableButton = within(modal!).getByText('Table').closest('button')
      fireEvent.click(tableButton!)

      // Check that the new cell has a unique name (using underscore separator)
      const callArg = onConfigChange.mock.calls[0][0]
      const newCell = callArg.cells[1]
      expect(newCell.name).toBe('Table_2')
    })
  })

  describe('cell selection', () => {
    it('should select cell when double-clicked', async () => {
      const cells = [createTableCell('Query')]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      // Find the cell container and double-click it
      const cellContainer = screen.getByText('Query').closest('[class*="group/cell"]')
      fireEvent.doubleClick(cellContainer!)

      // The editor panel should appear with cell name input
      expect(screen.getByText('Cell Name')).toBeInTheDocument()
    })

    it('should show editor panel when cell is selected', async () => {
      const cells = [createTableCell('My Query', 'SELECT * FROM logs')]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      const cellContainer = screen.getByText('My Query').closest('[class*="group/cell"]')
      fireEvent.doubleClick(cellContainer!)

      // Editor should show cell name in input
      expect(screen.getByDisplayValue('My Query')).toBeInTheDocument()
    })

    it('should close editor when close button is clicked', async () => {
      const cells = [createTableCell('Query')]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      // Select cell
      const cellContainer = screen.getByText('Query').closest('[class*="group/cell"]')
      fireEvent.doubleClick(cellContainer!)

      expect(screen.getByText('Cell Name')).toBeInTheDocument()

      // Click close button in editor - find the X icon in the editor panel (has border-l class)
      const editorPanel = screen.getByText('Cell Name').closest('div[class*="border-l"]')
      const closeButton = within(editorPanel!).getByTitle('Close')
      fireEvent.click(closeButton)

      expect(screen.queryByText('Cell Name')).not.toBeInTheDocument()
    })
  })

  describe('cell deletion', () => {
    it('should show delete confirmation modal', async () => {
      const cells = [createTableCell('ToDelete')]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      // Select the cell first
      const cellContainer = screen.getByText('ToDelete').closest('[class*="group/cell"]')
      fireEvent.doubleClick(cellContainer!)

      // Click delete in editor - look for "Delete Cell" button
      const deleteButton = screen.getByText('Delete Cell')
      fireEvent.click(deleteButton)

      expect(screen.getByText('Delete Cell?')).toBeInTheDocument()
      expect(screen.getByText(/Are you sure you want to delete "ToDelete"/)).toBeInTheDocument()
    })

    it('should delete cell when confirmed', async () => {
      const cells = [createTableCell('ToDelete')]
      const onConfigChange = vi.fn()

      await renderNotebook(
        createDefaultProps({
          config: { cells },
          onConfigChange,
        })
      )

      // Select cell
      const cellContainer = screen.getByText('ToDelete').closest('[class*="group/cell"]')
      fireEvent.doubleClick(cellContainer!)

      // Click delete in editor
      fireEvent.click(screen.getByText('Delete Cell'))

      // Confirm deletion - find the Delete button in the modal (the one with red background)
      const modal = screen.getByText('Delete Cell?').closest('div[class*="bg-app-panel"]')
      const confirmButton = within(modal!).getByRole('button', { name: 'Delete' })
      fireEvent.click(confirmButton)

      // onConfigChange should be called with empty cells
      expect(onConfigChange).toHaveBeenCalled()
    })

    it('should cancel deletion when cancel is clicked', async () => {
      const cells = [createTableCell('ToDelete')]
      const onConfigChange = vi.fn()

      await renderNotebook(
        createDefaultProps({
          config: { cells },
          onConfigChange,
        })
      )

      // Select cell - find the cell by name in the main content area
      const cellContainer = screen.getByText('ToDelete').closest('[class*="group/cell"]')
      fireEvent.doubleClick(cellContainer!)

      // Click delete in editor
      fireEvent.click(screen.getByText('Delete Cell'))

      // Cancel deletion
      fireEvent.click(screen.getByText('Cancel'))

      // Modal should close
      expect(screen.queryByText('Delete Cell?')).not.toBeInTheDocument()

      // Cell should still exist - check for the cell name in the sortable context
      const sortableContext = screen.getByTestId('sortable-context')
      expect(within(sortableContext).getByText('ToDelete')).toBeInTheDocument()
    })
  })

  describe('cell updates', () => {
    it('should update cell name through editor', async () => {
      const cells = [createTableCell('OldName')]
      const onConfigChange = vi.fn()

      await renderNotebook(
        createDefaultProps({
          config: { cells },
          onConfigChange,
        })
      )

      // Select cell
      const cellContainer = screen.getByText('OldName').closest('[class*="group/cell"]')
      fireEvent.doubleClick(cellContainer!)

      // Update name in editor
      const nameInput = screen.getByDisplayValue('OldName')
      fireEvent.change(nameInput, { target: { value: 'NewName' } })

      await waitFor(() => {
        expect(onConfigChange).toHaveBeenCalled()
      })
    })

    it('should show error for duplicate cell names', async () => {
      const cells = [createTableCell('First'), createTableCell('Second')]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      // Select second cell
      const cellContainer = screen.getByText('Second').closest('[class*="group/cell"]')
      fireEvent.doubleClick(cellContainer!)

      // Try to rename to existing name
      const nameInput = screen.getByDisplayValue('Second')
      fireEvent.change(nameInput, { target: { value: 'First' } })

      await waitFor(() => {
        expect(screen.getByText('A cell with this name already exists')).toBeInTheDocument()
      })
    })

    it('should show error for empty cell name', async () => {
      const cells = [createTableCell('Query')]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      // Select cell
      const cellContainer = screen.getByText('Query').closest('[class*="group/cell"]')
      fireEvent.doubleClick(cellContainer!)

      // Clear name
      const nameInput = screen.getByDisplayValue('Query')
      fireEvent.change(nameInput, { target: { value: '' } })

      await waitFor(() => {
        expect(screen.getByText('Cell name cannot be empty')).toBeInTheDocument()
      })
    })
  })

  describe('unsaved changes', () => {
    it('should expose save handler via onSaveRef', async () => {
      const cells = [createTableCell('Query')]
      const onSave = vi.fn().mockResolvedValue({ cells })
      const saveRef = { current: null } as React.MutableRefObject<(() => Promise<void>) | null>

      await renderNotebook(
        createDefaultProps({
          config: { cells },
          onSave,
          onSaveRef: saveRef,
        })
      )

      // The renderer should have set the ref to its wrapped save handler
      expect(saveRef.current).not.toBeNull()
      expect(typeof saveRef.current).toBe('function')
    })

    it('should not render save buttons in editor panel', async () => {
      const cells = [createTableCell('Query')]

      await renderNotebook(
        createDefaultProps({
          config: { cells },
        })
      )

      // Select a cell to show the editor panel
      const cellContainer = screen.getByText('Query').closest('[class*="group/cell"]')
      fireEvent.doubleClick(cellContainer!)

      // Save buttons should NOT be in the renderer (they're in the parent title bar now)
      const editorPanel = screen.getByText('Cell Name').closest('div[class*="border-l"]')
      expect(within(editorPanel!).queryByText('Save')).not.toBeInTheDocument()
    })
  })

  describe('cell execution', () => {
    it('should show run button for non-markdown cells', async () => {
      const cells = [createTableCell('Query')]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      expect(screen.getByTitle('Run cell')).toBeInTheDocument()
    })

    it('should show a run button for markdown cells (canRun via metadata, not execute)', async () => {
      const cells = [createMarkdownCell('Notes')]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      expect(screen.getByTitle('Run cell')).toBeInTheDocument()
    })

    it('should not show "Run from here" or "Auto-run from here" for markdown cells', async () => {
      const cells = [createMarkdownCell('Notes')]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      expect(screen.queryByText('Run from here')).not.toBeInTheDocument()
      expect(screen.queryByText('Auto-run from here')).not.toBeInTheDocument()
    })

    it('updates markdown output live when content changes while status is success, with no Run click', async () => {
      const cells = [createMarkdownCell('Notes', 'v1')]
      const props = createDefaultProps({ config: { cells } })
      const { rerender } = await renderNotebook(props)

      // Get the cell to 'success' via its own Run button (auto-execution on
      // mount depends on the WASM engine loading, which isn't available here).
      const notesRow = screen.getByText('Notes').closest('div')
      await act(async () => {
        fireEvent.click(within(notesRow!).getByTitle('Run cell'))
      })
      expect(screen.getByTestId('cell-renderer-markdown')).toHaveTextContent('v1')

      const updatedCells = [createMarkdownCell('Notes', 'v2')]
      await act(async () => {
        rerender(<NotebookRenderer {...props} config={{ cells: updatedCells }} />)
      })

      expect(screen.getByTestId('cell-renderer-markdown')).toHaveTextContent('v2')
    })

    it('renders a newly-added idle markdown cell only after its own Run is clicked, without executing any other cell', async () => {
      // Spy on the table cell type's execute so we can prove clicking markdown's
      // Run never triggers it (a query re-run would be the real cost this plan avoids).
      const tableMeta = CELL_TYPE_METADATA.table as unknown as {
        execute: (...args: unknown[]) => Promise<unknown>
      }
      const tableExecuteSpy = vi.spyOn(tableMeta, 'execute')

      const tableCell = createTableCell('Query')
      const props = createDefaultProps({ config: { cells: [tableCell] } })
      const { rerender } = await renderNotebook(props)

      // Run the table cell once via its own Run button to establish a baseline
      // call count (auto-execution on mount isn't exercised in this environment).
      await act(async () => {
        fireEvent.click(screen.getByTitle('Run cell'))
      })
      expect(tableExecuteSpy).toHaveBeenCalledTimes(1)

      // Simulate a markdown cell added after mount: it has no cellStates entry
      // yet, so it starts 'idle' and renders blank — same as a fresh add/duplicate.
      const markdownCell = createMarkdownCell('Notes', 'hello')
      await act(async () => {
        rerender(<NotebookRenderer {...props} config={{ cells: [tableCell, markdownCell] }} />)
      })

      const markdownRenderer = screen.getByTestId('cell-renderer-markdown')
      expect(markdownRenderer).toHaveTextContent('')

      const notesRow = screen.getByText('Notes').closest('div')
      const runButton = within(notesRow!).getByTitle('Run cell')
      await act(async () => {
        fireEvent.click(runButton)
      })

      expect(screen.getByTestId('cell-renderer-markdown')).toHaveTextContent('hello')
      // The markdown Run is a synchronous local no-op — it must not re-run the table query.
      expect(tableExecuteSpy).toHaveBeenCalledTimes(1)
    })
  })

  describe('cell menu', () => {
    it('should show menu with options', async () => {
      const cells = [createTableCell('Query')]

      await renderNotebook(createDefaultProps({ config: { cells } }))

      // Radix menu items render inline via the mock portal
      expect(screen.getByText('Run from here')).toBeInTheDocument()
      expect(screen.getByText('Delete cell')).toBeInTheDocument()
      expect(screen.getByText('Auto-run from here')).toBeInTheDocument()
    })
  })

  describe('collapsed cells', () => {
    it('should toggle collapsed state when chevron is clicked', async () => {
      const cells = [createTableCell('Query')]
      const onConfigChange = vi.fn()

      await renderNotebook(
        createDefaultProps({
          config: { cells },
          onConfigChange,
        })
      )

      // Click collapse toggle
      const toggleButton = screen.getByTestId('chevron-down').closest('button')
      fireEvent.click(toggleButton!)

      expect(onConfigChange).toHaveBeenCalled()
    })
  })

  describe('empty config handling', () => {
    it('should handle null config gracefully', async () => {
      await renderNotebook(createDefaultProps({ config: null as unknown as Record<string, unknown> }))

      expect(screen.getByText('Add Cell')).toBeInTheDocument()
    })

    it('should handle config without cells array', async () => {
      await renderNotebook(createDefaultProps({ config: {} }))

      expect(screen.getByText('Add Cell')).toBeInTheDocument()
    })
  })

  describe('time range selection', () => {
    it('should call onTimeRangeChange with ISO strings when cell triggers time range select', async () => {
      const onTimeRangeChange = vi.fn()
      const cells = [createChartCell('Metrics')]

      await renderNotebook(
        createDefaultProps({
          config: { cells },
          onTimeRangeChange,
        })
      )

      // The mock renderer triggers onTimeRangeSelect on click with hardcoded dates
      const cellRenderer = screen.getByTestId('cell-renderer-chart')
      fireEvent.click(cellRenderer)

      expect(onTimeRangeChange).toHaveBeenCalledWith(
        '2024-01-15T00:00:00.000Z',
        '2024-01-16T00:00:00.000Z'
      )
    })

    it('should pass onTimeRangeSelect to all cell types', async () => {
      const onTimeRangeChange = vi.fn()
      const cells = [
        createTableCell('Table'),
        createChartCell('Chart'),
        createMarkdownCell('Notes'),
      ]

      await renderNotebook(
        createDefaultProps({
          config: { cells },
          onTimeRangeChange,
        })
      )

      // Chart cell should have the callback wired up
      const chartRenderer = screen.getByTestId('cell-renderer-chart')
      fireEvent.click(chartRenderer)

      expect(onTimeRangeChange).toHaveBeenCalled()
    })
  })
})
