import { render, screen, fireEvent } from '@testing-library/react'
import { CellContainer } from '../CellContainer'

// Mock cell-registry to provide metadata
jest.mock('@/lib/screen-renderers/cell-registry', () => ({
  getCellTypeMetadata: (type: string) => {
    const metadata: Record<string, { label: string; showTypeBadge: boolean; execute?: () => void }> = {
      table: { label: 'Table', showTypeBadge: true, execute: () => {} },
      chart: { label: 'Chart', showTypeBadge: true, execute: () => {} },
      log: { label: 'Log', showTypeBadge: true, execute: () => {} },
      markdown: { label: 'Markdown', showTypeBadge: false },
      variable: { label: 'Variable', showTypeBadge: true, execute: () => {} },
    }
    return metadata[type] || { label: type, showTypeBadge: true, execute: () => {} }
  },
}))

// Mock lucide-react icons
jest.mock('lucide-react', () => ({
  ChevronDown: () => <span data-testid="chevron-down">▼</span>,
  ChevronRight: () => <span data-testid="chevron-right">▶</span>,
  Play: () => <span data-testid="play">▶</span>,
  RotateCcw: () => <span data-testid="rotate">↻</span>,
  MoreVertical: () => <span data-testid="more">⋮</span>,
  Trash2: () => <span data-testid="trash">🗑</span>,
  GripVertical: () => <span data-testid="grip">⠿</span>,
}))

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
      const onToggleCollapsed = jest.fn()
      render(<CellContainer {...defaultProps} onToggleCollapsed={onToggleCollapsed} />)

      // Find the collapse toggle button (contains chevron)
      const toggleButton = screen.getByTestId('chevron-down').closest('button')
      fireEvent.click(toggleButton!)

      expect(onToggleCollapsed).toHaveBeenCalledTimes(1)
    })

    it('should call onSelect when cell is clicked', () => {
      const onSelect = jest.fn()
      render(<CellContainer {...defaultProps} onSelect={onSelect} />)

      // Click on the cell container (not on a button)
      const cell = screen.getByText('Test Cell').closest('div[class*="bg-app-panel"]')
      fireEvent.click(cell!)

      expect(onSelect).toHaveBeenCalledTimes(1)
    })

    it('should call onRun when run button is clicked', () => {
      const onRun = jest.fn()
      render(<CellContainer {...defaultProps} onRun={onRun} />)

      // Find the run button (has title="Run cell")
      const runButton = screen.getByTitle('Run cell')
      fireEvent.click(runButton)

      expect(onRun).toHaveBeenCalledTimes(1)
    })

    it('should not call onSelect when run button is clicked', () => {
      const onSelect = jest.fn()
      const onRun = jest.fn()
      render(<CellContainer {...defaultProps} onSelect={onSelect} onRun={onRun} />)

      const runButton = screen.getByTitle('Run cell')
      fireEvent.click(runButton)

      // onRun should be called but onSelect should not (stopPropagation)
      expect(onRun).toHaveBeenCalledTimes(1)
      expect(onSelect).not.toHaveBeenCalled()
    })

    it('should disable run button when loading', () => {
      const onRun = jest.fn()
      render(<CellContainer {...defaultProps} status="loading" onRun={onRun} />)

      const runButton = screen.getByTitle('Run cell')
      expect(runButton).toBeDisabled()
    })

    it('should not show run button for markdown cells', () => {
      render(<CellContainer {...defaultProps} type="markdown" onRun={jest.fn()} />)
      expect(screen.queryByTitle('Run cell')).not.toBeInTheDocument()
    })
  })

  describe('menu', () => {
    it('should show menu when menu button is clicked', () => {
      render(
        <CellContainer
          {...defaultProps}
          onRunFromHere={jest.fn()}
          onDelete={jest.fn()}
        />
      )

      // Menu should not be visible initially
      expect(screen.queryByText('Run from here')).not.toBeInTheDocument()

      // Click menu button
      const menuButton = screen.getByTestId('more').closest('button')
      fireEvent.click(menuButton!)

      // Menu should now be visible
      expect(screen.getByText('Run from here')).toBeInTheDocument()
      expect(screen.getByText('Delete cell')).toBeInTheDocument()
    })

    it('should call onRunFromHere when menu item is clicked', () => {
      const onRunFromHere = jest.fn()
      render(
        <CellContainer
          {...defaultProps}
          onRunFromHere={onRunFromHere}
          onDelete={jest.fn()}
        />
      )

      // Open menu
      const menuButton = screen.getByTestId('more').closest('button')
      fireEvent.click(menuButton!)

      // Click "Run from here"
      fireEvent.click(screen.getByText('Run from here'))

      expect(onRunFromHere).toHaveBeenCalledTimes(1)
    })

    it('should call onDelete when delete menu item is clicked', () => {
      const onDelete = jest.fn()
      render(
        <CellContainer
          {...defaultProps}
          onRunFromHere={jest.fn()}
          onDelete={onDelete}
        />
      )

      // Open menu
      const menuButton = screen.getByTestId('more').closest('button')
      fireEvent.click(menuButton!)

      // Click "Delete cell"
      fireEvent.click(screen.getByText('Delete cell'))

      expect(onDelete).toHaveBeenCalledTimes(1)
    })

    it('should not show "Run from here" for markdown cells', () => {
      render(
        <CellContainer
          {...defaultProps}
          type="markdown"
          onRunFromHere={jest.fn()}
          onDelete={jest.fn()}
        />
      )

      // Open menu
      const menuButton = screen.getByTestId('more').closest('button')
      fireEvent.click(menuButton!)

      expect(screen.queryByText('Run from here')).not.toBeInTheDocument()
      expect(screen.getByText('Delete cell')).toBeInTheDocument()
    })

    it('should close menu when clicking outside', () => {
      render(
        <CellContainer
          {...defaultProps}
          onRunFromHere={jest.fn()}
          onDelete={jest.fn()}
        />
      )

      // Open menu
      const menuButton = screen.getByTestId('more').closest('button')
      fireEvent.click(menuButton!)
      expect(screen.getByText('Run from here')).toBeInTheDocument()

      // Click outside (on document body)
      fireEvent.mouseDown(document.body)

      // Menu should close
      expect(screen.queryByText('Run from here')).not.toBeInTheDocument()
    })
  })

  describe('selection state', () => {
    it('should apply selected styles when isSelected is true', () => {
      const { container } = render(<CellContainer {...defaultProps} isSelected={true} />)

      // Check for selected class
      const cell = container.firstChild as HTMLElement
      expect(cell.className).toContain('border-accent-link')
    })

    it('should not apply selected styles when isSelected is false', () => {
      const { container } = render(<CellContainer {...defaultProps} isSelected={false} />)

      const cell = container.firstChild as HTMLElement
      expect(cell.className).not.toContain('border-accent-link')
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
      const onHeightChange = jest.fn()
      render(<CellContainer {...defaultProps} onHeightChange={onHeightChange} />)

      // ResizeHandle has role="separator"
      expect(screen.getByRole('separator')).toBeInTheDocument()
    })

    it('should not render resize handle when onHeightChange is not provided', () => {
      render(<CellContainer {...defaultProps} />)

      expect(screen.queryByRole('separator')).not.toBeInTheDocument()
    })

    it('should not render resize handle when collapsed', () => {
      const onHeightChange = jest.fn()
      render(<CellContainer {...defaultProps} collapsed={true} onHeightChange={onHeightChange} />)

      expect(screen.queryByRole('separator')).not.toBeInTheDocument()
    })
  })
})
