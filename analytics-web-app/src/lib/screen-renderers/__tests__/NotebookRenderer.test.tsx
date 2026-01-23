/**
 * Tests for NotebookRenderer component
 */
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react'
import React from 'react'

// Mock streamQuery to prevent actual API calls
const mockStreamQuery = jest.fn()
jest.mock('@/lib/arrow-stream', () => ({
  streamQuery: (...args: unknown[]) => mockStreamQuery(...args),
}))

// Mock Apache Arrow
jest.mock('apache-arrow', () => ({
  Table: class MockTable {
    numRows = 0
    numCols = 0
    constructor(public batches: unknown[] = []) {}
  },
}))

// Mock lucide-react icons
jest.mock('lucide-react', () => ({
  Plus: () => <span data-testid="plus-icon">+</span>,
  X: () => <span data-testid="x-icon">×</span>,
  ChevronDown: () => <span data-testid="chevron-down">▼</span>,
  ChevronRight: () => <span data-testid="chevron-right">▶</span>,
  Play: () => <span data-testid="play">▶</span>,
  RotateCcw: () => <span data-testid="rotate">↻</span>,
  MoreVertical: () => <span data-testid="more">⋮</span>,
  Trash2: () => <span data-testid="trash">🗑</span>,
  GripVertical: () => <span data-testid="grip">⠿</span>,
  Settings: () => <span data-testid="settings">⚙</span>,
  Save: () => <span data-testid="save">💾</span>,
}))

// Mock @dnd-kit to simplify testing
jest.mock('@dnd-kit/core', () => ({
  DndContext: ({ children }: { children: React.ReactNode }) => <div data-testid="dnd-context">{children}</div>,
  closestCenter: jest.fn(),
  KeyboardSensor: jest.fn(),
  PointerSensor: jest.fn(),
  useSensor: jest.fn(() => ({})),
  useSensors: jest.fn(() => []),
  DragOverlay: ({ children }: { children: React.ReactNode }) => <div data-testid="drag-overlay">{children}</div>,
}))

jest.mock('@dnd-kit/sortable', () => ({
  arrayMove: (arr: unknown[], from: number, to: number) => {
    const result = [...arr] as unknown[]
    const [removed] = result.splice(from, 1)
    result.splice(to, 0, removed)
    return result
  },
  SortableContext: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="sortable-context">{children}</div>
  ),
  sortableKeyboardCoordinates: jest.fn(),
  useSortable: () => ({
    attributes: {},
    listeners: {},
    setNodeRef: jest.fn(),
    transform: null,
    transition: null,
    isDragging: false,
  }),
  verticalListSortingStrategy: jest.fn(),
}))

jest.mock('@dnd-kit/utilities', () => ({
  CSS: {
    Transform: {
      toString: () => '',
    },
  },
}))

// Mock the cell registry
// eslint-disable-next-line @typescript-eslint/no-var-requires
jest.mock('../cell-registry', () => require('../__test-utils__/cell-registry-mock').createCellRegistryMock({ withRenderers: true, withEditors: true }))

// Import after mocks are set up
import { NotebookRenderer } from '../NotebookRenderer'
import { ScreenRendererProps } from '../index'
import { CellConfig } from '../notebook-utils'

// Helper to create default props
function createDefaultProps(overrides: Partial<ScreenRendererProps> = {}): ScreenRendererProps {
  return {
    config: { cells: [] },
    onConfigChange: jest.fn(),
    savedConfig: { cells: [] },
    setHasUnsavedChanges: jest.fn(),
    timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
    onSave: jest.fn(),
    isSaving: false,
    hasUnsavedChanges: false,
    onSaveAs: jest.fn(),
    saveError: null,
    refreshTrigger: 0,
    ...overrides,
  }
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
  variableType: 'text' | 'number' | 'combobox' = 'text'
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

describe('NotebookRenderer', () => {
  beforeEach(() => {
    jest.clearAllMocks()
    // Default mock for successful queries - synchronously return done
    mockStreamQuery.mockImplementation(async function* () {
      yield { type: 'done' }
    })
  })

  describe('initial rendering', () => {
    it('should render empty notebook with add cell button', () => {
      render(<NotebookRenderer {...createDefaultProps()} />)

      expect(screen.getByText('Add Cell')).toBeInTheDocument()
    })

    it('should render cells from config', () => {
      const cells = [createTableCell('Query 1'), createTableCell('Query 2')]

      render(<NotebookRenderer {...createDefaultProps({ config: { cells } })} />)

      expect(screen.getByText('Query 1')).toBeInTheDocument()
      expect(screen.getByText('Query 2')).toBeInTheDocument()
    })

    it('should render different cell types', () => {
      const cells = [
        createTableCell('MyTable'),
        createMarkdownCell('Notes'),
        createVariableCell('Filter'),
      ]

      render(<NotebookRenderer {...createDefaultProps({ config: { cells } })} />)

      expect(screen.getByText('MyTable')).toBeInTheDocument()
      expect(screen.getByText('Notes')).toBeInTheDocument()
      expect(screen.getByText('Filter')).toBeInTheDocument()
    })
  })

  describe('add cell modal', () => {
    it('should open add cell modal when add button is clicked', () => {
      render(<NotebookRenderer {...createDefaultProps()} />)

      fireEvent.click(screen.getByText('Add Cell'))

      expect(screen.getByText('Table')).toBeInTheDocument()
      expect(screen.getByText('Chart')).toBeInTheDocument()
      expect(screen.getByText('Log')).toBeInTheDocument()
      expect(screen.getByText('Markdown')).toBeInTheDocument()
      expect(screen.getByText('Variable')).toBeInTheDocument()
    })

    it('should close modal when X button is clicked', () => {
      render(<NotebookRenderer {...createDefaultProps()} />)

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
      const onConfigChange = jest.fn()
      const setHasUnsavedChanges = jest.fn()

      render(
        <NotebookRenderer
          {...createDefaultProps({
            onConfigChange,
            setHasUnsavedChanges,
          })}
        />
      )

      fireEvent.click(screen.getByText('Add Cell'))

      // Find the Table button in the modal (not the badge)
      const modal = screen.getByRole('heading', { name: 'Add Cell' }).closest('div[class*="bg-app-panel"]')
      const tableButton = within(modal!).getByText('Table').closest('button')
      fireEvent.click(tableButton!)

      expect(onConfigChange).toHaveBeenCalled()
      expect(setHasUnsavedChanges).toHaveBeenCalled()

      // Modal should close after adding
      expect(screen.queryByRole('heading', { name: 'Add Cell' })).not.toBeInTheDocument()
    })

    it('should generate unique names for new cells', () => {
      const existingCells = [createTableCell('Table')]
      const onConfigChange = jest.fn()

      render(
        <NotebookRenderer
          {...createDefaultProps({
            config: { cells: existingCells },
            onConfigChange,
          })}
        />
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
    it('should select cell when clicked', () => {
      const cells = [createTableCell('Query')]

      render(<NotebookRenderer {...createDefaultProps({ config: { cells } })} />)

      // Find the cell container and click it
      const cellContainer = screen.getByText('Query').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cellContainer!)

      // The editor panel should appear with cell name input
      expect(screen.getByText('Cell Name')).toBeInTheDocument()
    })

    it('should show editor panel when cell is selected', () => {
      const cells = [createTableCell('My Query', 'SELECT * FROM logs')]

      render(<NotebookRenderer {...createDefaultProps({ config: { cells } })} />)

      const cellContainer = screen.getByText('My Query').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cellContainer!)

      // Editor should show cell name in input
      expect(screen.getByDisplayValue('My Query')).toBeInTheDocument()
    })

    it('should close editor when close button is clicked', () => {
      const cells = [createTableCell('Query')]

      render(<NotebookRenderer {...createDefaultProps({ config: { cells } })} />)

      // Select cell
      const cellContainer = screen.getByText('Query').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cellContainer!)

      expect(screen.getByText('Cell Name')).toBeInTheDocument()

      // Click close button in editor - find the X icon in the editor panel (has border-l class)
      const editorPanel = screen.getByText('Cell Name').closest('div[class*="border-l"]')
      const closeButton = within(editorPanel!).getByTitle('Close')
      fireEvent.click(closeButton)

      expect(screen.queryByText('Cell Name')).not.toBeInTheDocument()
    })
  })

  describe('cell deletion', () => {
    it('should show delete confirmation modal', () => {
      const cells = [createTableCell('ToDelete')]

      render(<NotebookRenderer {...createDefaultProps({ config: { cells } })} />)

      // Select the cell first
      const cellContainer = screen.getByText('ToDelete').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cellContainer!)

      // Click delete in editor - look for "Delete Cell" button
      const deleteButton = screen.getByText('Delete Cell')
      fireEvent.click(deleteButton)

      expect(screen.getByText('Delete Cell?')).toBeInTheDocument()
      expect(screen.getByText(/Are you sure you want to delete "ToDelete"/)).toBeInTheDocument()
    })

    it('should delete cell when confirmed', () => {
      const cells = [createTableCell('ToDelete')]
      const onConfigChange = jest.fn()

      render(
        <NotebookRenderer
          {...createDefaultProps({
            config: { cells },
            onConfigChange,
          })}
        />
      )

      // Select cell
      const cellContainer = screen.getByText('ToDelete').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cellContainer!)

      // Click delete in editor
      fireEvent.click(screen.getByText('Delete Cell'))

      // Confirm deletion - find the Delete button in the modal (the one with red background)
      const modal = screen.getByText('Delete Cell?').closest('div[class*="bg-app-panel"]')
      const confirmButton = within(modal!).getByRole('button', { name: 'Delete' })
      fireEvent.click(confirmButton)

      // onConfigChange should be called with empty cells
      expect(onConfigChange).toHaveBeenCalled()
    })

    it('should cancel deletion when cancel is clicked', () => {
      const cells = [createTableCell('ToDelete')]
      const onConfigChange = jest.fn()

      render(
        <NotebookRenderer
          {...createDefaultProps({
            config: { cells },
            onConfigChange,
          })}
        />
      )

      // Select cell - find the cell by name in the main content area
      const cellContainer = screen.getByText('ToDelete').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cellContainer!)

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
      const onConfigChange = jest.fn()
      const setHasUnsavedChanges = jest.fn()

      render(
        <NotebookRenderer
          {...createDefaultProps({
            config: { cells },
            onConfigChange,
            setHasUnsavedChanges,
          })}
        />
      )

      // Select cell
      const cellContainer = screen.getByText('OldName').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cellContainer!)

      // Update name in editor
      const nameInput = screen.getByDisplayValue('OldName')
      fireEvent.change(nameInput, { target: { value: 'NewName' } })

      await waitFor(() => {
        expect(onConfigChange).toHaveBeenCalled()
      })
    })

    it('should show error for duplicate cell names', async () => {
      const cells = [createTableCell('First'), createTableCell('Second')]

      render(<NotebookRenderer {...createDefaultProps({ config: { cells } })} />)

      // Select second cell
      const cellContainer = screen.getByText('Second').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cellContainer!)

      // Try to rename to existing name
      const nameInput = screen.getByDisplayValue('Second')
      fireEvent.change(nameInput, { target: { value: 'First' } })

      await waitFor(() => {
        expect(screen.getByText('A cell with this name already exists')).toBeInTheDocument()
      })
    })

    it('should show error for empty cell name', async () => {
      const cells = [createTableCell('Query')]

      render(<NotebookRenderer {...createDefaultProps({ config: { cells } })} />)

      // Select cell
      const cellContainer = screen.getByText('Query').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cellContainer!)

      // Clear name
      const nameInput = screen.getByDisplayValue('Query')
      fireEvent.change(nameInput, { target: { value: '' } })

      await waitFor(() => {
        expect(screen.getByText('Cell name cannot be empty')).toBeInTheDocument()
      })
    })
  })

  describe('unsaved changes', () => {
    it('should show save footer when hasUnsavedChanges is true', () => {
      const cells = [createTableCell('Query')]

      render(
        <NotebookRenderer
          {...createDefaultProps({
            config: { cells },
            hasUnsavedChanges: true,
          })}
        />
      )

      // Select a cell to show the editor panel with save footer
      const cellContainer = screen.getByText('Query').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cellContainer!)

      expect(screen.getByText('Save')).toBeInTheDocument()
    })

    it('should call onSave when save button is clicked', () => {
      const cells = [createTableCell('Query')]
      const onSave = jest.fn()

      render(
        <NotebookRenderer
          {...createDefaultProps({
            config: { cells },
            hasUnsavedChanges: true,
            onSave,
          })}
        />
      )

      // Select cell to show editor
      const cellContainer = screen.getByText('Query').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cellContainer!)

      fireEvent.click(screen.getByText('Save'))

      expect(onSave).toHaveBeenCalled()
    })
  })

  describe('cell execution', () => {
    it('should show run button for non-markdown cells', () => {
      const cells = [createTableCell('Query')]

      render(<NotebookRenderer {...createDefaultProps({ config: { cells } })} />)

      expect(screen.getByTitle('Run cell')).toBeInTheDocument()
    })

    it('should not show run button for markdown cells', () => {
      const cells = [createMarkdownCell('Notes')]

      render(<NotebookRenderer {...createDefaultProps({ config: { cells } })} />)

      expect(screen.queryByTitle('Run cell')).not.toBeInTheDocument()
    })
  })

  describe('cell menu', () => {
    it('should show menu with options when menu button is clicked', () => {
      const cells = [createTableCell('Query')]

      render(<NotebookRenderer {...createDefaultProps({ config: { cells } })} />)

      // Click menu button
      const menuButton = screen.getByTestId('more').closest('button')
      fireEvent.click(menuButton!)

      expect(screen.getByText('Run from here')).toBeInTheDocument()
      expect(screen.getByText('Delete cell')).toBeInTheDocument()
    })
  })

  describe('collapsed cells', () => {
    it('should toggle collapsed state when chevron is clicked', () => {
      const cells = [createTableCell('Query')]
      const onConfigChange = jest.fn()

      render(
        <NotebookRenderer
          {...createDefaultProps({
            config: { cells },
            onConfigChange,
          })}
        />
      )

      // Click collapse toggle
      const toggleButton = screen.getByTestId('chevron-down').closest('button')
      fireEvent.click(toggleButton!)

      expect(onConfigChange).toHaveBeenCalled()
    })
  })

  describe('empty config handling', () => {
    it('should handle null config gracefully', () => {
      render(<NotebookRenderer {...createDefaultProps({ config: null as unknown as Record<string, unknown> })} />)

      expect(screen.getByText('Add Cell')).toBeInTheDocument()
    })

    it('should handle config without cells array', () => {
      render(<NotebookRenderer {...createDefaultProps({ config: {} })} />)

      expect(screen.getByText('Add Cell')).toBeInTheDocument()
    })
  })
})
