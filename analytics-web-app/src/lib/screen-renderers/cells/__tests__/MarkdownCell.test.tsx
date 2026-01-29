import { render, screen } from '@testing-library/react'
import { MarkdownCell } from '../MarkdownCell'
import { CellRendererProps } from '../../cell-registry'

// Helper to create mock props
function createMockProps(overrides: Partial<CellRendererProps> = {}): CellRendererProps {
  return {
    name: 'test-cell',
    data: null,
    status: 'success',
    timeRange: { begin: '2024-01-01', end: '2024-01-02' },
    variables: {},
    isEditing: false,
    onRun: jest.fn(),
    onSqlChange: jest.fn(),
    onOptionsChange: jest.fn(),
    ...overrides,
  }
}

describe('MarkdownCell', () => {
  describe('headers', () => {
    it('should render h1 headers', () => {
      render(<MarkdownCell {...createMockProps({ content: '# Hello World' })} />)
      expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('Hello World')
    })

    it('should render h2 headers', () => {
      render(<MarkdownCell {...createMockProps({ content: '## Section Title' })} />)
      expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent('Section Title')
    })

    it('should render h3 headers', () => {
      render(<MarkdownCell {...createMockProps({ content: '### Subsection' })} />)
      expect(screen.getByRole('heading', { level: 3 })).toHaveTextContent('Subsection')
    })
  })

  describe('paragraphs', () => {
    it('should render plain text as paragraph', () => {
      render(<MarkdownCell {...createMockProps({ content: 'This is a paragraph.' })} />)
      expect(screen.getByText('This is a paragraph.')).toBeInTheDocument()
    })

    it('should render multiple paragraphs', () => {
      render(
        <MarkdownCell {...createMockProps({ content: 'First paragraph.\n\nSecond paragraph.' })} />
      )
      expect(screen.getByText('First paragraph.')).toBeInTheDocument()
      expect(screen.getByText('Second paragraph.')).toBeInTheDocument()
    })
  })

  describe('inline formatting', () => {
    it('should render bold text', () => {
      render(<MarkdownCell {...createMockProps({ content: 'This is **bold** text.' })} />)
      const boldElement = screen.getByText('bold')
      expect(boldElement.tagName.toLowerCase()).toBe('strong')
    })

    it('should render italic text', () => {
      render(<MarkdownCell {...createMockProps({ content: 'This is *italic* text.' })} />)
      const italicElement = screen.getByText('italic')
      expect(italicElement.tagName.toLowerCase()).toBe('em')
    })

    it('should render inline code', () => {
      render(<MarkdownCell {...createMockProps({ content: 'Use `const` for constants.' })} />)
      const codeElement = screen.getByText('const')
      expect(codeElement.tagName.toLowerCase()).toBe('code')
    })

    it('should render links', () => {
      render(
        <MarkdownCell {...createMockProps({ content: 'Visit [Example](https://example.com)' })} />
      )
      const link = screen.getByRole('link', { name: 'Example' })
      expect(link).toHaveAttribute('href', 'https://example.com')
    })
  })

  describe('lists', () => {
    it('should render unordered list', () => {
      render(<MarkdownCell {...createMockProps({ content: '- Item 1\n- Item 2\n- Item 3' })} />)
      expect(screen.getByRole('list')).toBeInTheDocument()
      expect(screen.getAllByRole('listitem')).toHaveLength(3)
    })

    it('should render ordered list', () => {
      render(<MarkdownCell {...createMockProps({ content: '1. First\n2. Second\n3. Third' })} />)
      expect(screen.getByRole('list')).toBeInTheDocument()
      expect(screen.getAllByRole('listitem')).toHaveLength(3)
    })

    it('should render task lists (GFM)', () => {
      render(
        <MarkdownCell {...createMockProps({ content: '- [ ] Todo\n- [x] Done' })} />
      )
      const checkboxes = screen.getAllByRole('checkbox')
      expect(checkboxes).toHaveLength(2)
      expect(checkboxes[0]).not.toBeChecked()
      expect(checkboxes[1]).toBeChecked()
    })
  })

  describe('code blocks', () => {
    it('should render fenced code blocks', () => {
      render(
        <MarkdownCell {...createMockProps({ content: '```\nconst x = 1;\n```' })} />
      )
      expect(screen.getByText('const x = 1;')).toBeInTheDocument()
    })
  })

  describe('tables (GFM)', () => {
    it('should render tables', () => {
      const tableMarkdown = `| Name | Age |
| ---- | --- |
| Alice | 30 |
| Bob | 25 |`
      render(<MarkdownCell {...createMockProps({ content: tableMarkdown })} />)
      expect(screen.getAllByRole('table').length).toBeGreaterThan(0)
      expect(screen.getByText('Alice')).toBeInTheDocument()
      expect(screen.getByText('Bob')).toBeInTheDocument()
    })
  })

  describe('blockquotes', () => {
    it('should render blockquotes', () => {
      render(<MarkdownCell {...createMockProps({ content: '> This is a quote' })} />)
      expect(screen.getByText('This is a quote')).toBeInTheDocument()
    })
  })

  describe('edge cases', () => {
    it('should handle empty content', () => {
      const { container } = render(<MarkdownCell {...createMockProps({ content: '' })} />)
      expect(container.querySelector('.prose')).toBeInTheDocument()
    })

    it('should handle undefined content', () => {
      const { container } = render(<MarkdownCell {...createMockProps({ content: undefined })} />)
      expect(container.querySelector('.prose')).toBeInTheDocument()
    })
  })

  describe('variable substitution', () => {
    it('should substitute simple string variables', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: 'Selected metric: $metric',
            variables: { metric: 'cpu_usage' },
          })}
        />
      )
      expect(screen.getByText('Selected metric: cpu_usage')).toBeInTheDocument()
    })

    it('should substitute $variable.column for multi-column variables', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: 'Metric: $metric.name ($metric.unit)',
            variables: { metric: { name: 'DeltaTime', unit: 'seconds' } },
          })}
        />
      )
      expect(screen.getByText('Metric: DeltaTime (seconds)')).toBeInTheDocument()
    })

    it('should substitute $begin and $end time range variables', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: 'Time range: $begin to $end',
            timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
          })}
        />
      )
      expect(
        screen.getByText("Time range: '2024-01-01T00:00:00Z' to '2024-01-02T00:00:00Z'")
      ).toBeInTheDocument()
    })

    it('should leave unresolved variables unchanged', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: 'Unknown: $unknown_var',
            variables: {},
          })}
        />
      )
      expect(screen.getByText('Unknown: $unknown_var')).toBeInTheDocument()
    })
  })
})
