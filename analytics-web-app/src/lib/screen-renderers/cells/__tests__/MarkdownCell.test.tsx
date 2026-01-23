import { render, screen } from '@testing-library/react'
import { MarkdownCell } from '../MarkdownCell'
import { CellRendererProps } from '../../cell-registry'

// Create a minimal mock for required CellRendererProps
const createMockProps = (overrides: Partial<CellRendererProps> = {}): CellRendererProps => ({
  name: 'test-markdown',
  sql: undefined,
  options: undefined,
  data: null,
  status: 'success',
  error: undefined,
  timeRange: { begin: '2024-01-01', end: '2024-01-02' },
  variables: {},
  isEditing: false,
  onRun: jest.fn(),
  onSqlChange: jest.fn(),
  onOptionsChange: jest.fn(),
  ...overrides,
})

describe('MarkdownCell', () => {
  describe('headers', () => {
    it('should render h1 headers', () => {
      render(<MarkdownCell {...createMockProps({ content: '# Main Title' })} />)
      const heading = screen.getByRole('heading', { level: 1 })
      expect(heading).toHaveTextContent('Main Title')
    })

    it('should render h2 headers', () => {
      render(<MarkdownCell {...createMockProps({ content: '## Section Title' })} />)
      const heading = screen.getByRole('heading', { level: 2 })
      expect(heading).toHaveTextContent('Section Title')
    })

    it('should render h3 headers', () => {
      render(<MarkdownCell {...createMockProps({ content: '### Subsection' })} />)
      const heading = screen.getByRole('heading', { level: 3 })
      expect(heading).toHaveTextContent('Subsection')
    })

    it('should render multiple header levels', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: '# Title\n## Section\n### Subsection',
          })}
        />
      )
      expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('Title')
      expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent('Section')
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
        <MarkdownCell
          {...createMockProps({
            content: 'First paragraph.\n\nSecond paragraph.',
          })}
        />
      )
      expect(screen.getByText('First paragraph.')).toBeInTheDocument()
      expect(screen.getByText('Second paragraph.')).toBeInTheDocument()
    })
  })

  describe('inline formatting', () => {
    it('should render bold text', () => {
      render(<MarkdownCell {...createMockProps({ content: 'This is **bold** text.' })} />)
      const boldElement = screen.getByText('bold')
      expect(boldElement.tagName).toBe('STRONG')
    })

    it('should render italic text', () => {
      render(<MarkdownCell {...createMockProps({ content: 'This is *italic* text.' })} />)
      const italicElement = screen.getByText('italic')
      expect(italicElement.tagName).toBe('EM')
    })

    it('should render inline code', () => {
      render(<MarkdownCell {...createMockProps({ content: 'Use `code` here.' })} />)
      const codeElement = screen.getByText('code')
      expect(codeElement.tagName).toBe('CODE')
    })

    it('should render links', () => {
      render(
        <MarkdownCell
          {...createMockProps({ content: 'Visit [Example](https://example.com).' })}
        />
      )
      const link = screen.getByRole('link', { name: 'Example' })
      expect(link).toHaveAttribute('href', 'https://example.com')
      expect(link).toHaveAttribute('target', '_blank')
      expect(link).toHaveAttribute('rel', 'noopener noreferrer')
    })

    it('should handle multiple inline formats in one line', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: 'This has **bold** and *italic* and `code`.',
          })}
        />
      )
      expect(screen.getByText('bold').tagName).toBe('STRONG')
      expect(screen.getByText('italic').tagName).toBe('EM')
      expect(screen.getByText('code').tagName).toBe('CODE')
    })
  })

  describe('lists', () => {
    it('should render unordered list with dash', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: '- Item 1\n- Item 2\n- Item 3',
          })}
        />
      )
      const list = screen.getByRole('list')
      expect(list.tagName).toBe('UL')
      expect(screen.getAllByRole('listitem')).toHaveLength(3)
    })

    it('should render unordered list with asterisk', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: '* Item A\n* Item B',
          })}
        />
      )
      expect(screen.getAllByRole('listitem')).toHaveLength(2)
    })

    it('should render list item content', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: '- First item\n- Second item',
          })}
        />
      )
      expect(screen.getByText('First item')).toBeInTheDocument()
      expect(screen.getByText('Second item')).toBeInTheDocument()
    })

    it('should support inline formatting in list items', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: '- Item with **bold**\n- Item with *italic*',
          })}
        />
      )
      expect(screen.getByText('bold').tagName).toBe('STRONG')
      expect(screen.getByText('italic').tagName).toBe('EM')
    })
  })

  describe('complex documents', () => {
    it('should render a full document with mixed content', () => {
      const content = `# Documentation

This is an introduction paragraph.

## Features

- Feature **one**
- Feature *two*
- Feature with \`code\`

### Links

Visit [our docs](https://docs.example.com) for more.`

      render(<MarkdownCell {...createMockProps({ content })} />)

      // Check headers
      expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('Documentation')
      expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent('Features')
      expect(screen.getByRole('heading', { level: 3 })).toHaveTextContent('Links')

      // Check paragraph
      expect(screen.getByText('This is an introduction paragraph.')).toBeInTheDocument()

      // Check list
      expect(screen.getAllByRole('listitem')).toHaveLength(3)

      // Check inline formatting
      expect(screen.getByText('one').tagName).toBe('STRONG')
      expect(screen.getByText('two').tagName).toBe('EM')
      expect(screen.getByText('code').tagName).toBe('CODE')

      // Check link
      expect(screen.getByRole('link', { name: 'our docs' })).toHaveAttribute(
        'href',
        'https://docs.example.com'
      )
    })
  })

  describe('edge cases', () => {
    it('should handle empty content', () => {
      const { container } = render(<MarkdownCell {...createMockProps({ content: '' })} />)
      // Should render without crashing
      expect(container).toBeInTheDocument()
    })

    it('should handle undefined content', () => {
      const { container } = render(
        <MarkdownCell {...createMockProps({ content: undefined })} />
      )
      expect(container).toBeInTheDocument()
    })

    it('should handle content with only whitespace', () => {
      const { container } = render(
        <MarkdownCell {...createMockProps({ content: '   \n\n   ' })} />
      )
      expect(container).toBeInTheDocument()
    })

    it('should handle header without space after hash', () => {
      // This should be treated as regular text, not a header
      render(<MarkdownCell {...createMockProps({ content: '#NoSpace' })} />)
      expect(screen.queryByRole('heading')).not.toBeInTheDocument()
    })
  })

  describe('edit mode', () => {
    it('should show preview label in edit mode', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: '# Title',
            isEditing: true,
            onContentChange: jest.fn(),
          })}
        />
      )
      expect(screen.getByText('Preview:')).toBeInTheDocument()
    })

    it('should render markdown preview in edit mode', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: '# Title',
            isEditing: true,
            onContentChange: jest.fn(),
          })}
        />
      )
      expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('Title')
    })

    it('should not show preview label in view mode', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: '# Title',
            isEditing: false,
          })}
        />
      )
      expect(screen.queryByText('Preview:')).not.toBeInTheDocument()
    })
  })
})
