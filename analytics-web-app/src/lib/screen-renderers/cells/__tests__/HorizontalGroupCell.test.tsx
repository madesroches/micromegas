import { render, screen, fireEvent, within } from '@testing-library/react'
import React from 'react'

// Mock lucide-react icons
jest.mock('lucide-react', () => ({
  GripVertical: () => <span data-testid="grip">⠿</span>,
  Play: () => <span data-testid="play">▶</span>,
  RotateCcw: () => <span data-testid="rotate">↻</span>,
  MoreVertical: () => <span data-testid="more">⋮</span>,
  Trash2: () => <span data-testid="trash">🗑</span>,
  ChevronLeft: () => <span data-testid="chevron-left">◀</span>,
  ChevronRight: () => <span data-testid="chevron-right">▶</span>,
  Plus: () => <span data-testid="plus">+</span>,
  X: () => <span data-testid="x-icon">×</span>,
  ArrowLeft: () => <span data-testid="arrow-left">←</span>,
}))

// Mock @dnd-kit
jest.mock('@dnd-kit/core', () => ({
  DndContext: ({ children }: { children: React.ReactNode }) => <div data-testid="dnd-context">{children}</div>,
  closestCenter: jest.fn(),
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
  useSortable: () => ({
    attributes: { 'data-testid': 'drag-handle-attrs' },
    listeners: {},
    setNodeRef: jest.fn(),
    transform: null,
    transition: null,
    isDragging: false,
  }),
  horizontalListSortingStrategy: jest.fn(),
}))

jest.mock('@dnd-kit/utilities', () => ({
  CSS: {
    Transform: {
      toString: () => '',
    },
  },
}))

// Mock data-sources-api (used by DataSourceSelector)
jest.mock('@/lib/data-sources-api', () => ({
  getDataSourceList: jest.fn().mockReturnValue(new Promise(() => {})),
}))

// Mock cell-registry
jest.mock('../../cell-registry', () =>
  require('../../__test-utils__/cell-registry-mock').createCellRegistryMock({
    withRenderers: true,
    withEditors: true,
  })
)

import {
  HorizontalGroupCell,
  HorizontalGroupCellEditor,
  HorizontalGroupCellProps,
} from '../HorizontalGroupCell'
import type {
  CellConfig,
  HorizontalGroupCellConfig,
} from '../../notebook-types'

// =============================================================================
// Helpers
// =============================================================================

function makeChild(
  name: string,
  type: CellConfig['type'] = 'table',
  overrides: Partial<CellConfig> = {},
): CellConfig {
  return {
    type,
    name,
    sql: type === 'table' || type === 'chart' || type === 'log' ? 'SELECT 1' : undefined,
    content: type === 'markdown' ? '# Hello' : undefined,
    layout: { height: 300 },
    ...overrides,
  } as CellConfig
}

function createRendererProps(overrides: Partial<HorizontalGroupCellProps> = {}): HorizontalGroupCellProps {
  return {
    config: {
      type: 'hg',
      name: 'group1',
      layout: { height: 300 },
      children: [],
    },
    cellStates: {},
    variables: {},
    variableValues: {},
    timeRange: { begin: '2024-01-01', end: '2024-01-02' },
    selectedChildName: null,
    onChildSelect: jest.fn(),
    onChildRun: jest.fn(),
    onVariableValueChange: jest.fn(),
    onConfigChange: jest.fn(),
    onChildDragOut: jest.fn(),
    allCellNames: new Set(['group1']),
    ...overrides,
  }
}

interface EditorOverrides {
  config?: HorizontalGroupCellConfig
  onChange?: jest.Mock
  selectedChildName?: string | null
  onChildSelect?: jest.Mock
  variables?: Record<string, string>
  timeRange?: { begin: string; end: string }
  allCellNames?: Set<string>
  availableColumns?: string[]
  datasourceVariables?: string[]
  defaultDataSource?: string
  showNotebookOption?: boolean
}

function createEditorProps(overrides: EditorOverrides = {}) {
  return {
    config: overrides.config || {
      type: 'hg' as const,
      name: 'group1',
      layout: { height: 300 },
      children: [],
    },
    onChange: overrides.onChange || jest.fn(),
    selectedChildName: overrides.selectedChildName ?? null,
    onChildSelect: overrides.onChildSelect || jest.fn(),
    variables: overrides.variables || {},
    timeRange: overrides.timeRange || { begin: '2024-01-01', end: '2024-01-02' },
    allCellNames: overrides.allCellNames || new Set(['group1']),
    availableColumns: overrides.availableColumns,
    datasourceVariables: overrides.datasourceVariables,
    defaultDataSource: overrides.defaultDataSource,
    showNotebookOption: overrides.showNotebookOption,
  }
}

// =============================================================================
// Tests: HorizontalGroupCell (Renderer)
// =============================================================================

describe('HorizontalGroupCell', () => {
  describe('rendering', () => {
    it('shows placeholder when group has no children', () => {
      render(<HorizontalGroupCell {...createRendererProps()} />)
      expect(screen.getByText('Add cells to this group from the editor panel')).toBeInTheDocument()
    })

    it('renders child names in headers', () => {
      const children = [makeChild('chart_a', 'chart'), makeChild('table_b', 'table')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
          })}
        />
      )
      expect(screen.getByText('chart_a')).toBeInTheDocument()
      expect(screen.getByText('table_b')).toBeInTheDocument()
    })

    it('renders cell content via CellRenderer for each child', () => {
      const children = [makeChild('q1', 'table'), makeChild('q2', 'chart')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
          })}
        />
      )
      expect(screen.getByTestId('cell-renderer-table')).toBeInTheDocument()
      expect(screen.getByTestId('cell-renderer-chart')).toBeInTheDocument()
    })

    it('shows error state for children with errors', () => {
      const children = [makeChild('err_cell', 'table')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            cellStates: { err_cell: { status: 'error', error: 'Something broke', data: [] } },
          })}
        />
      )
      expect(screen.getByText('Error:')).toBeInTheDocument()
      expect(screen.getByText('Something broke')).toBeInTheDocument()
    })

    it('shows blocked state for blocked children', () => {
      const children = [makeChild('blocked_cell', 'table')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            cellStates: { blocked_cell: { status: 'blocked', data: [] } },
          })}
        />
      )
      expect(screen.getByText('Waiting for cell above to succeed')).toBeInTheDocument()
    })

    it('selected child gets selection border styling', () => {
      const children = [makeChild('sel', 'table')]
      const { container } = render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            selectedChildName: 'sel',
          })}
        />
      )
      const selBorder = container.querySelector('.border-\\[var\\(--selection-border\\)\\]')
      expect(selBorder).toBeInTheDocument()
    })

    it('shows spinner icon and Running text for loading child', () => {
      const children = [makeChild('loading_cell', 'table')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            cellStates: { loading_cell: { status: 'loading', data: [] } },
          })}
        />
      )
      expect(screen.getByTestId('rotate')).toBeInTheDocument()
      expect(screen.getByText('Running...')).toBeInTheDocument()
    })

    it('shows status text when present', () => {
      const children = [makeChild('status_cell', 'table')]
      const mockTable = {
        numRows: 5,
        numCols: 2,
        batches: [{ data: { byteLength: 1024 } }],
      }
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            cellStates: { status_cell: { status: 'success', data: [mockTable as never] } },
          })}
        />
      )
      // buildStatusText will produce a row count string
      expect(screen.getByText(/5 rows/)).toBeInTheDocument()
    })
  })

  describe('interactions', () => {
    it('click child header calls onChildSelect with child name', () => {
      const onChildSelect = jest.fn()
      const children = [makeChild('clickme', 'table')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            onChildSelect,
          })}
        />
      )
      fireEvent.click(screen.getByText('clickme'))
      expect(onChildSelect).toHaveBeenCalledWith('clickme')
    })

    it('click run button calls onChildRun with child name', () => {
      const onChildRun = jest.fn()
      const children = [makeChild('runme', 'table')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            onChildRun,
          })}
        />
      )
      const runButton = screen.getByTitle('Run cell')
      fireEvent.click(runButton)
      expect(onChildRun).toHaveBeenCalledWith('runme')
    })

    it('run button is disabled during loading', () => {
      const children = [makeChild('busy', 'table')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            cellStates: { busy: { status: 'loading', data: [] } },
          })}
        />
      )
      const runButton = screen.getByTitle('Run cell')
      expect(runButton).toBeDisabled()
    })

    it('click "Remove from group" calls onConfigChange with child removed', () => {
      const onConfigChange = jest.fn()
      const children = [makeChild('a', 'table'), makeChild('b', 'chart')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            onConfigChange,
          })}
        />
      )
      // The Radix mock renders DropdownMenu.Item as a <button> with onClick=onSelect
      const removeButtons = screen.getAllByText('Remove from group')
      fireEvent.click(removeButtons[0])
      expect(onConfigChange).toHaveBeenCalledWith(
        expect.objectContaining({
          children: [expect.objectContaining({ name: 'b' })],
        })
      )
    })

    it('removing selected child also calls onChildSelect(null)', () => {
      const onChildSelect = jest.fn()
      const onConfigChange = jest.fn()
      const children = [makeChild('selected', 'table')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            selectedChildName: 'selected',
            onChildSelect,
            onConfigChange,
          })}
        />
      )
      fireEvent.click(screen.getByText('Remove from group'))
      expect(onChildSelect).toHaveBeenCalledWith(null)
    })

    it('stopPropagation on drag handle click', () => {
      const onChildSelect = jest.fn()
      const children = [makeChild('draggable', 'table')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            onChildSelect,
          })}
        />
      )
      // The grip icon is inside the drag handle button
      const gripButton = screen.getByTestId('grip').closest('button')!
      fireEvent.click(gripButton)
      // onChildSelect should NOT have been called because stopPropagation prevents it
      expect(onChildSelect).not.toHaveBeenCalled()
    })
  })

  describe('drag-drop structure', () => {
    it('DndContext and SortableContext are rendered', () => {
      const children = [makeChild('a', 'table')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
          })}
        />
      )
      expect(screen.getByTestId('dnd-context')).toBeInTheDocument()
      expect(screen.getByTestId('sortable-context')).toBeInTheDocument()
    })

    it('DragOverlay is rendered', () => {
      const children = [makeChild('a', 'table')]
      render(
        <HorizontalGroupCell
          {...createRendererProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
          })}
        />
      )
      expect(screen.getByTestId('drag-overlay')).toBeInTheDocument()
    })

    it('not rendered for empty group', () => {
      render(<HorizontalGroupCell {...createRendererProps()} />)
      expect(screen.queryByTestId('dnd-context')).not.toBeInTheDocument()
    })
  })
})

// =============================================================================
// Tests: HorizontalGroupCellEditor (Group View)
// =============================================================================

describe('HorizontalGroupCellEditor', () => {
  describe('group view', () => {
    it('shows children count label', () => {
      const children = [makeChild('a', 'table'), makeChild('b', 'chart')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
          })}
        />
      )
      expect(screen.getByText('Children (2)')).toBeInTheDocument()
    })

    it('shows "No children yet" for empty group', () => {
      render(<HorizontalGroupCellEditor {...createEditorProps()} />)
      expect(screen.getByText('No children yet')).toBeInTheDocument()
    })

    it('lists children with names', () => {
      const children = [makeChild('alpha', 'table'), makeChild('beta', 'log')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
          })}
        />
      )
      expect(screen.getByText('alpha')).toBeInTheDocument()
      expect(screen.getByText('beta')).toBeInTheDocument()
    })

    it('click child name calls onChildSelect', () => {
      const onChildSelect = jest.fn()
      const children = [makeChild('clickable', 'table')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            onChildSelect,
          })}
        />
      )
      fireEvent.click(screen.getByText('clickable'))
      expect(onChildSelect).toHaveBeenCalledWith('clickable')
    })

    it('move left reorders children', () => {
      const onChange = jest.fn()
      const children = [makeChild('first', 'table'), makeChild('second', 'chart')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            onChange,
          })}
        />
      )
      // "Move left" buttons - first child's is disabled, second child's is enabled
      const moveLeftButtons = screen.getAllByTitle('Move left')
      fireEvent.click(moveLeftButtons[1]) // click second child's move left
      expect(onChange).toHaveBeenCalledWith(
        expect.objectContaining({
          children: [
            expect.objectContaining({ name: 'second' }),
            expect.objectContaining({ name: 'first' }),
          ],
        })
      )
    })

    it('move left disabled on first child', () => {
      const children = [makeChild('only', 'table'), makeChild('other', 'chart')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
          })}
        />
      )
      const moveLeftButtons = screen.getAllByTitle('Move left')
      expect(moveLeftButtons[0]).toBeDisabled()
    })

    it('move right disabled on last child', () => {
      const children = [makeChild('first', 'table'), makeChild('last', 'chart')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
          })}
        />
      )
      const moveRightButtons = screen.getAllByTitle('Move right')
      expect(moveRightButtons[moveRightButtons.length - 1]).toBeDisabled()
    })

    it('remove button removes child from config', () => {
      const onChange = jest.fn()
      const children = [makeChild('keep', 'table'), makeChild('remove_me', 'chart')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            onChange,
          })}
        />
      )
      const removeButtons = screen.getAllByTitle('Remove')
      fireEvent.click(removeButtons[1]) // remove second child
      expect(onChange).toHaveBeenCalledWith(
        expect.objectContaining({
          children: [expect.objectContaining({ name: 'keep' })],
        })
      )
    })

    it('"Add Child Cell" button opens modal', () => {
      render(<HorizontalGroupCellEditor {...createEditorProps()} />)
      fireEvent.click(screen.getByText('Add Child Cell'))
      expect(screen.getByRole('heading', { name: 'Add Child Cell' })).toBeInTheDocument()
    })
  })

  describe('child editor view', () => {
    it('shows "Back to group" button that calls onChildSelect(null)', () => {
      const onChildSelect = jest.fn()
      const children = [makeChild('editable', 'table')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            selectedChildName: 'editable',
            onChildSelect,
          })}
        />
      )
      const backButton = screen.getByText('Back to group')
      fireEvent.click(backButton)
      expect(onChildSelect).toHaveBeenCalledWith(null)
    })

    it('shows child name input field', () => {
      const children = [makeChild('my_cell', 'table')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            selectedChildName: 'my_cell',
          })}
        />
      )
      expect(screen.getByDisplayValue('my_cell')).toBeInTheDocument()
    })

    it('name validation shows error for duplicates', () => {
      const children = [makeChild('alpha', 'table'), makeChild('beta', 'chart')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            selectedChildName: 'beta',
            allCellNames: new Set(['group1', 'alpha', 'beta']),
          })}
        />
      )
      const nameInput = screen.getByDisplayValue('beta')
      fireEvent.change(nameInput, { target: { value: 'alpha' } })
      expect(screen.getByText('A cell with this name already exists')).toBeInTheDocument()
    })

    it('name change updates config with sanitized name', () => {
      const onChange = jest.fn()
      const onChildSelect = jest.fn()
      const children = [makeChild('old_name', 'table')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            selectedChildName: 'old_name',
            onChange,
            onChildSelect,
          })}
        />
      )
      const nameInput = screen.getByDisplayValue('old_name')
      fireEvent.change(nameInput, { target: { value: 'new_name' } })
      expect(onChange).toHaveBeenCalledWith(
        expect.objectContaining({
          children: [expect.objectContaining({ name: 'new_name' })],
        })
      )
      expect(onChildSelect).toHaveBeenCalledWith('new_name')
    })

    it('renders type-specific EditorComponent', () => {
      const children = [makeChild('edit_me', 'table')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children },
            selectedChildName: 'edit_me',
          })}
        />
      )
      expect(screen.getByTestId('editor-table')).toBeInTheDocument()
    })

    it('DataSourceField not shown for markdown type', () => {
      // DataSourceField only renders when data sources are loaded;
      // verify that shouldShowDataSource logic excludes markdown by checking
      // that the editor for markdown does NOT attempt to render DataSourceField at all
      const mdChildren = [makeChild('notes', 'markdown')]
      render(
        <HorizontalGroupCellEditor
          {...createEditorProps({
            config: { type: 'hg', name: 'g', layout: { height: 300 }, children: mdChildren },
            selectedChildName: 'notes',
          })}
        />
      )
      expect(screen.queryByText('Data Source')).not.toBeInTheDocument()
    })
  })
})

// =============================================================================
// Tests: AddChildModal
// =============================================================================

describe('AddChildModal (via HorizontalGroupCellEditor)', () => {
  it('modal not rendered when closed', () => {
    render(<HorizontalGroupCellEditor {...createEditorProps()} />)
    expect(screen.queryByRole('heading', { name: 'Add Child Cell' })).not.toBeInTheDocument()
  })

  it('shows cell type options excluding hg', () => {
    render(<HorizontalGroupCellEditor {...createEditorProps()} />)
    fireEvent.click(screen.getByText('Add Child Cell'))

    const modal = screen.getByRole('heading', { name: 'Add Child Cell' }).closest('div[class*="bg-app-panel"]')!
    expect(within(modal).getByText('Table')).toBeInTheDocument()
    expect(within(modal).getByText('Chart')).toBeInTheDocument()
    expect(within(modal).getByText('Log')).toBeInTheDocument()
    expect(within(modal).getByText('Markdown')).toBeInTheDocument()
    expect(within(modal).getByText('Variable')).toBeInTheDocument()
    // 'hg' (Group) should NOT appear
    expect(within(modal).queryByText('Group')).not.toBeInTheDocument()
  })

  it('clicking option calls onChange with new child added', () => {
    const onChange = jest.fn()
    render(<HorizontalGroupCellEditor {...createEditorProps({ onChange })} />)
    fireEvent.click(screen.getByText('Add Child Cell'))

    const modal = screen.getByRole('heading', { name: 'Add Child Cell' }).closest('div[class*="bg-app-panel"]')!
    const tableButton = within(modal).getByText('Table').closest('button')!
    fireEvent.click(tableButton)

    expect(onChange).toHaveBeenCalled()
    const newConfig = onChange.mock.calls[0][0]
    expect(newConfig.children).toHaveLength(1)
    expect(newConfig.children[0].type).toBe('table')
  })

  it('backdrop click closes modal', () => {
    render(<HorizontalGroupCellEditor {...createEditorProps()} />)
    fireEvent.click(screen.getByText('Add Child Cell'))
    expect(screen.getByRole('heading', { name: 'Add Child Cell' })).toBeInTheDocument()

    // Click the backdrop (bg-black/50 div)
    const backdrop = screen.getByRole('heading', { name: 'Add Child Cell' })
      .closest('.fixed')!
      .querySelector('.bg-black\\/50')!
    fireEvent.click(backdrop)

    expect(screen.queryByRole('heading', { name: 'Add Child Cell' })).not.toBeInTheDocument()
  })
})
