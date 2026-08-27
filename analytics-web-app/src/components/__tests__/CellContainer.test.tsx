import { render, screen, fireEvent } from '@testing-library/react'
import { CellContainer } from '../CellContainer'

// Mock cell-registry to provide metadata
vi.mock('@/lib/screen-renderers/cell-registry', async () => {
  const { createCellRegistryMock } = await import('@/lib/screen-renderers/__test-utils__/cell-registry-mock')
  return createCellRegistryMock()
})

// Mock lucide-react icons
vi.mock('lucide-react', () => ({
  ChevronDown: () => <span data-testid="chevron-down">▼</span>,
  ChevronRight: () => <span data-testid="chevron-right">▶</span>,
  Download: () => <span data-testid="download">⬇</span>,
  Play: () => <span data-testid="play">▶</span>,
  RotateCcw: () => <span data-testid="rotate">↻</span>,
  MoreVertical: () => <span data-testid="more">⋮</span>,
  Trash2: () => <span data-testid="trash">🗑</span>,
  GripVertical: () => <span data-testid="grip">⠿</span>,
  Zap: () => <span data-testid="zap">⚡</span>,
  Copy: () => <span data-testid="copy">📋</span>,
  Pencil: () => <span data-testid="pencil">✏</span>,
}))

// @radix-ui/react-dropdown-menu is mocked via test.alias in vite.config.ts

describe('CellContainer', () => {
  const defaultProps = {
    name: 'Test Cell',
    type: 'table' as const,
    status: 'success' as const,
    children: <div>Cell content</div>,
  }

  describe('rendering', () => {
    it('should render cell name', () => {
      render(<CellContainer {...defaultProps} />)
      expect(screen.getByText('Test Cell')).toBeInTheDocument()
    })

    it('should render cell type badge', () => {
      render(<CellContainer {...defaultProps} />)
      expect(screen.getByText('Table')).toBeInTheDocument()
    })

    it('should render children when not collapsed', () => {
      render(<CellContainer {...defaultProps} />)
      expect(screen.getByText('Cell content')).toBeInTheDocument()
    })

    it('should not render children when collapsed', () => {
      render(<CellContainer {...defaultProps} collapsed={true} />)
      expect(screen.queryByText('Cell content')).not.toBeInTheDocument()
    })

    it('should render all cell type badges correctly', () => {
      // Non-markdown cells show type badge
      const typesWithBadge = ['table', 'chart', 'log', 'variable'] as const
      const labels = ['Table', 'Chart', 'Log', 'Variable']

      typesWithBadge.forEach((type, index) => {
        const { unmount } = render(
          <CellContainer {...defaultProps} type={type} />
        )
        expect(screen.getByText(labels[index])).toBeInTheDocument()
        unmount()
      })
    })

    it('should show cell name instead of type badge for markdown cells', () => {
      render(<CellContainer {...defaultProps} type="markdown" name="My Notes" />)
      expect(screen.getByText('My Notes')).toBeInTheDocument()
      expect(screen.queryByText('Markdown')).not.toBeInTheDocument()
    })
  })

  describe('status display', () => {
    it('should show "Running..." when loading', () => {
      render(<CellContainer {...defaultProps} status="loading" />)
      expect(screen.getByText('Running...')).toBeInTheDocument()
    })

    it('should show "Error" when status is error', () => {
      render(<CellContainer {...defaultProps} status="error" />)
      expect(screen.getByText('Error')).toBeInTheDocument()
    })

    it('should show "Blocked" when status is blocked', () => {
      render(<CellContainer {...defaultProps} status="blocked" />)
      expect(screen.getByText('Blocked')).toBeInTheDocument()
    })

    it('should show custom status text when provided', () => {
      render(<CellContainer {...defaultProps} statusText="15 rows" />)
      expect(screen.getByText('15 rows')).toBeInTheDocument()
    })

    it('should show error message when status is error', () => {
      render(
        <CellContainer {...defaultProps} status="error" error="Query failed" />
      )
      expect(screen.getByText('Query failed')).toBeInTheDocument()
      expect(screen.getByText('Query execution failed')).toBeInTheDocument()
    })

    it('should show blocked message when status is blocked', () => {
      render(<CellContainer {...defaultProps} status="blocked" />)
      expect(screen.getByText('Waiting for cell above to succeed')).toBeInTheDocument()
    })
  })

  describe('interactions', () => {
    it('should call onToggleCollapsed when collapse button is clicked', () => {
      const onToggleCollapsed = vi.fn()
      render(<CellContainer {...defaultProps} onToggleCollapsed={onToggleCollapsed} />)

      // Find the collapse toggle button (contains chevron)
      const toggleButton = screen.getByTestId('chevron-down').closest('button')
      fireEvent.click(toggleButton!)

      expect(onToggleCollapsed).toHaveBeenCalledTimes(1)
    })

    it('should call onSelect when cell is double-clicked', () => {
      const onSelect = vi.fn()
      const { container } = render(<CellContainer {...defaultProps} onSelect={onSelect} />)

      // Double-click on the cell container root
      const cell = container.firstChild as HTMLElement
      fireEvent.doubleClick(cell)

      expect(onSelect).toHaveBeenCalledTimes(1)
    })

    it('should call onRun when run button is clicked', () => {
      const onRun = vi.fn()
      render(<CellContainer {...defaultProps} onRun={onRun} />)

      // Find the run button (has title="Run cell")
      const runButton = screen.getByTitle('Run cell')
      fireEvent.click(runButton)

      expect(onRun).toHaveBeenCalledTimes(1)
    })

    it('should not call onSelect when run button is clicked', () => {
      const onSelect = vi.fn()
      const onRun = vi.fn()
      render(<CellContainer {...defaultProps} onSelect={onSelect} onRun={onRun} />)

      const runButton = screen.getByTitle('Run cell')
      fireEvent.click(runButton)

      // onRun should be called but onSelect should not
      expect(onRun).toHaveBeenCalledTimes(1)
      expect(onSelect).not.toHaveBeenCalled()
    })

    it('should disable run button when loading', () => {
      const onRun = vi.fn()
      render(<CellContainer {...defaultProps} status="loading" onRun={onRun} />)

      const runButton = screen.getByTitle('Run cell')
      expect(runButton).toBeDisabled()
    })

    it('should show run button for a type with no execute but canRun: true (e.g. markdown)', () => {
      // The shared mock registry gives markdown `canRun: true` with no `execute`,
      // mirroring production — this is the metadata-driven fallback, not canRunProp.
      render(<CellContainer {...defaultProps} type="markdown" onRun={vi.fn()} />)
      expect(screen.getByTitle('Run cell')).toBeInTheDocument()
    })

    it('should hide the run button when canRunProp={false} regardless of type', () => {
      render(<CellContainer {...defaultProps} onRun={vi.fn()} canRun={false} />)
      expect(screen.queryByTitle('Run cell')).not.toBeInTheDocument()
    })
  })

  describe('menu', () => {
    it('should render menu items for runnable cells', () => {
      render(
        <CellContainer
          {...defaultProps}
          onRunFromHere={vi.fn()}
          onDelete={vi.fn()}
        />
      )

      // Radix menu items are rendered (portal is mocked to inline)
      expect(screen.getByText('Run from here')).toBeInTheDocument()
      expect(screen.getByText('Delete cell')).toBeInTheDocument()
    })

    it('should call onRunFromHere when menu item is clicked', () => {
      const onRunFromHere = vi.fn()
      render(
        <CellContainer
          {...defaultProps}
          onRunFromHere={onRunFromHere}
          onDelete={vi.fn()}
        />
      )

      fireEvent.click(screen.getByText('Run from here'))

      expect(onRunFromHere).toHaveBeenCalledTimes(1)
    })

    it('should call onDelete when delete menu item is clicked', () => {
      const onDelete = vi.fn()
      render(
        <CellContainer
          {...defaultProps}
          onRunFromHere={vi.fn()}
          onDelete={onDelete}
        />
      )

      fireEvent.click(screen.getByText('Delete cell'))

      expect(onDelete).toHaveBeenCalledTimes(1)
    })

    it('should show auto-run toggle for runnable cells', () => {
      render(
        <CellContainer
          {...defaultProps}
          onToggleAutoRunFromHere={vi.fn()}
          onDelete={vi.fn()}
        />
      )

      expect(screen.getByText('Auto-run from here')).toBeInTheDocument()
    })

    it('should call onToggleAutoRunFromHere when auto-run item is clicked', () => {
      const onToggle = vi.fn()
      render(
        <CellContainer
          {...defaultProps}
          onToggleAutoRunFromHere={onToggle}
          onDelete={vi.fn()}
        />
      )

      fireEvent.click(screen.getByText('Auto-run from here'))

      expect(onToggle).toHaveBeenCalledTimes(1)
    })

    it('should show "Disable auto-run" when autoRunFromHere is true', () => {
      render(
        <CellContainer
          {...defaultProps}
          autoRunFromHere={true}
          onToggleAutoRunFromHere={vi.fn()}
          onDelete={vi.fn()}
        />
      )

      expect(screen.getByText('Disable auto-run')).toBeInTheDocument()
    })

    it('should show Zap indicator in header when autoRunFromHere is true', () => {
      render(
        <CellContainer
          {...defaultProps}
          autoRunFromHere={true}
          onToggleAutoRunFromHere={vi.fn()}
        />
      )

      expect(screen.getByTitle('Auto-run from here')).toBeInTheDocument()
    })
  })

  describe('selection state', () => {
    it('should apply selected styles when isSelected is true', () => {
      const { container } = render(<CellContainer {...defaultProps} isSelected={true} />)

      // Check for left accent bar selection indicator
      const cell = container.firstChild as HTMLElement
      expect(cell.className).toContain('border-l-accent-link')
    })

    it('should not apply selected styles when isSelected is false', () => {
      const { container } = render(<CellContainer {...defaultProps} isSelected={false} />)

      const cell = container.firstChild as HTMLElement
      expect(cell.className).not.toContain('border-l-accent-link')
    })
  })

  describe('drag handle', () => {
    it('should render drag handle when dragHandleProps are provided', () => {
      render(<CellContainer {...defaultProps} dragHandleProps={{}} />)
      expect(screen.getByTestId('grip')).toBeInTheDocument()
    })

    it('should not render drag handle when dragHandleProps are not provided', () => {
      render(<CellContainer {...defaultProps} />)
      expect(screen.queryByTestId('grip')).not.toBeInTheDocument()
    })

    it('should apply opacity when isDragging is true', () => {
      const { container } = render(<CellContainer {...defaultProps} isDragging={true} />)

      const cell = container.firstChild as HTMLElement
      expect(cell.className).toContain('opacity-50')
    })
  })

  describe('height setting', () => {
    it('should apply fixed height when height is provided', () => {
      render(<CellContainer {...defaultProps} height={300} />)

      // Find the content div (p-4 class)
      const contentDiv = screen.getByText('Cell content').parentElement
      expect(contentDiv?.style.height).toBe('300px')
    })

    it('should use default height when not specified', () => {
      render(<CellContainer {...defaultProps} />)

      const contentDiv = screen.getByText('Cell content').parentElement
      // Default height is 300px
      expect(contentDiv?.style.height).toBe('300px')
    })

    it('should render resize handle when onHeightChange is provided', () => {
      const onHeightChange = vi.fn()
      render(<CellContainer {...defaultProps} onHeightChange={onHeightChange} />)

      // ResizeHandle has role="separator"
      expect(screen.getByRole('separator')).toBeInTheDocument()
    })

    it('should not render resize handle when onHeightChange is not provided', () => {
      render(<CellContainer {...defaultProps} />)

      expect(screen.queryByRole('separator')).not.toBeInTheDocument()
    })

    it('should not render resize handle when collapsed', () => {
      const onHeightChange = vi.fn()
      render(<CellContainer {...defaultProps} collapsed={true} onHeightChange={onHeightChange} />)

      expect(screen.queryByRole('separator')).not.toBeInTheDocument()
    })
  })
})
