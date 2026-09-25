// Mock matchMedia for uPlot (imported via cell-registry -> ChartCell -> XYChart).
// Only needed by the 'calls the real createDefaultCell' test below, which is the
// only test in this file that imports the real (unmocked) cell-registry.
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: vi.fn().mockImplementation((query) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
})

import { render, screen, fireEvent } from '@testing-library/react'
import { tableFromArrays, Uint32, Utf8, Bool, FixedSizeBinary, type DataType, type Table } from 'apache-arrow'
import {
  MarkdownCell,
  markdownMetadata,
  resolveMarkdownColors,
  fitFontSize,
  MIN_FIT_FONT_PX,
  MAX_FIT_FONT_PX,
} from '../MarkdownCell'
import type { CellRendererProps, CellEditorProps } from '../../cell-registry'
import type { MarkdownCellConfig } from '../../notebook-types'

// Helper to create mock props
function createMockProps(overrides: Partial<CellRendererProps> = {}): CellRendererProps {
  return {
    name: 'test-cell',
    data: [],
    status: 'success',
    timeRange: { begin: '2024-01-01', end: '2024-01-02' },
    variables: {},
    isEditing: false,
    onRun: vi.fn(),
    onSqlChange: vi.fn(),
    onOptionsChange: vi.fn(),
    cellResults: {},
    cellSelections: {},
    ...overrides,
  }
}

function makeTable(columns: Record<string, unknown[]>): Table {
  return tableFromArrays(columns)
}

/** A minimal fake `Table` — only `schema.fields` is read by `resolveMarkdownColors`,
 *  which takes row values already extracted, so no real column data is needed. */
function fakeTable(fields: { name: string; type: DataType }[]): Table {
  return { schema: { fields } } as unknown as Table
}

describe('MarkdownCell', () => {
  describe('metadata', () => {
    it('has an execute method and canBlockDownstream: true, like other query-backed cell types', () => {
      expect(markdownMetadata.execute).toBeDefined()
      expect(markdownMetadata.canBlockDownstream).toBe(true)
    })

    it('createDefaultConfig includes the default SQL', () => {
      const config = markdownMetadata.createDefaultConfig() as MarkdownCellConfig
      expect(config.sql).toBe('SELECT 1')
    })

    it('calls the real createDefaultCell and gets dataSource: notebook even under a remote notebook default', async () => {
      // Dynamic import (not a static one) so it runs after this file's own
      // top-level matchMedia stub — the real cell-registry pulls in every cell
      // module, including ChartCell's uPlot, which reads matchMedia at import time.
      const { createDefaultCell } = await import('../../cell-registry')
      const cell = createDefaultCell('markdown', new Set(), 'remote') as MarkdownCellConfig
      expect(cell.dataSource).toBe('notebook')
      expect(cell.sql).toBe('SELECT 1')
    })
  })

  describe('execute', () => {
    const baseCtx = {
      variables: {},
      timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
      cellResults: {},
      cellSelections: {},
      cellDataSource: 'notebook',
    }

    it('runs config.sql after macro substitution', async () => {
      const runQuery = vi.fn().mockResolvedValue(makeTable({ value: [1] }))
      const config = { type: 'markdown', name: 'Stat', layout: { height: 150 }, content: '', sql: "SELECT '$metric' as value" } as MarkdownCellConfig
      await markdownMetadata.execute!(config, { ...baseCtx, variables: { metric: 'cpu' }, runQuery })
      expect(runQuery).toHaveBeenCalledWith("SELECT 'cpu' as value")
    })

    it('runs SELECT 1 when sql is absent', async () => {
      const runQuery = vi.fn().mockResolvedValue(makeTable({ n: [1] }))
      const config = { type: 'markdown', name: 'Stat', layout: { height: 150 }, content: '' } as MarkdownCellConfig
      await markdownMetadata.execute!(config, { ...baseCtx, runQuery })
      expect(runQuery).toHaveBeenCalledWith('SELECT 1')
    })

    it('runs SELECT 1 when sql is blank', async () => {
      const runQuery = vi.fn().mockResolvedValue(makeTable({ n: [1] }))
      const config = { type: 'markdown', name: 'Stat', layout: { height: 150 }, content: '', sql: '   ' } as MarkdownCellConfig
      await markdownMetadata.execute!(config, { ...baseCtx, runQuery })
      expect(runQuery).toHaveBeenCalledWith('SELECT 1')
    })

    it('throws on a zero-row result', async () => {
      const runQuery = vi.fn().mockResolvedValue(makeTable({ n: [] }))
      const config = { type: 'markdown', name: 'Stat', layout: { height: 150 }, content: '', sql: 'SELECT 1' } as MarkdownCellConfig
      await expect(markdownMetadata.execute!(config, { ...baseCtx, runQuery })).rejects.toThrow(
        'Query returned no rows'
      )
    })

    it('returns the table when there are several rows', async () => {
      const runQuery = vi.fn().mockResolvedValue(makeTable({ n: [1, 2, 3] }))
      const config = { type: 'markdown', name: 'Stat', layout: { height: 150 }, content: '', sql: 'SELECT 1' } as MarkdownCellConfig
      const result = await markdownMetadata.execute!(config, { ...baseCtx, runQuery })
      expect(result?.data?.[0].numRows).toBe(3)
    })
  })

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

  describe('deferred render', () => {
    it('should not render content when status is idle', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: '# Heading\n\nBody text.',
            status: 'idle',
          })}
        />
      )
      expect(screen.queryByRole('heading')).not.toBeInTheDocument()
      expect(screen.queryByText('Body text.')).not.toBeInTheDocument()
    })

    it('should not render content when status is blocked', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: '# Heading',
            status: 'blocked',
          })}
        />
      )
      expect(screen.queryByRole('heading')).not.toBeInTheDocument()
    })

    it('should not substitute macros when status is idle', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: 'value: $var',
            variables: { var: 'resolved' },
            status: 'idle',
          })}
        />
      )
      expect(screen.queryByText(/resolved/)).not.toBeInTheDocument()
      expect(screen.queryByText(/\$var/)).not.toBeInTheDocument()
    })

    it('keeps the previous output while idle or loading with data (a re-run in progress)', () => {
      const table = makeTable({ value: [1] })
      const { rerender } = render(
        <MarkdownCell {...createMockProps({ content: '$value', data: [table], status: 'success' })} />
      )
      expect(screen.getByText('1')).toBeInTheDocument()

      rerender(<MarkdownCell {...createMockProps({ content: '$value', data: [table], status: 'loading' })} />)
      expect(screen.getByText('1')).toBeInTheDocument()

      rerender(<MarkdownCell {...createMockProps({ content: '$value', data: [table], status: 'idle' })} />)
      expect(screen.getByText('1')).toBeInTheDocument()
    })

    it('shows the cached resolved value for an upstream $cell[0].col macro while idle-with-data, instead of re-evaluating against a stripped cellResults', () => {
      const upstream = makeTable({ col: ['resolved-value'] })
      const table = makeTable({ value: [1] })
      const content = 'Value: $upstream[0].col'
      const { rerender } = render(
        <MarkdownCell
          {...createMockProps({ content, data: [table], status: 'success', cellResults: { upstream } })}
        />
      )
      expect(screen.getByText('Value: resolved-value')).toBeInTheDocument()

      // executeFromCell resets this cell (and upstream) to idle up front, and
      // getAvailableCellResults only includes upstream cells with status
      // 'success' — so by the time this cell is idle-with-data, 'upstream' is
      // absent from cellResults. The cached evaluation must still be shown.
      rerender(
        <MarkdownCell {...createMockProps({ content, data: [table], status: 'idle', cellResults: {} })} />
      )
      expect(screen.getByText('Value: resolved-value')).toBeInTheDocument()
    })
  })

  describe('row-0 binding', () => {
    it('resolves a bare $col from row 0, winning over a same-named variable', () => {
      const table = makeTable({ value: [42] })
      render(
        <MarkdownCell
          {...createMockProps({ content: '$value', data: [table], variables: { value: 'from-variable' } })}
        />
      )
      expect(screen.getByText('42')).toBeInTheDocument()
      expect(screen.queryByText('from-variable')).not.toBeInTheDocument()
    })

    it('formats the raw row value with format_value', () => {
      const table = makeTable({ duration_ns: [2_500_000_000] })
      render(
        <MarkdownCell {...createMockProps({ content: "format_value($duration_ns, 'nanoseconds')", data: [table] })} />
      )
      expect(screen.getByText('2.50 seconds')).toBeInTheDocument()
    })

    it('ignores rows past 0', () => {
      const table = makeTable({ value: [1, 2, 3] })
      render(<MarkdownCell {...createMockProps({ content: '$value', data: [table] })} />)
      expect(screen.getByText('1')).toBeInTheDocument()
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

    it('should substitute $from and $to time range variables', () => {
      render(
        <MarkdownCell
          {...createMockProps({
            content: 'Time range: $from to $to',
            timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
          })}
        />
      )
      expect(
        screen.getByText('Time range: 2024-01-01T00:00:00Z to 2024-01-02T00:00:00Z')
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

  describe('resolveMarkdownColors', () => {
    it('decodes an integer color column', () => {
      const table = fakeTable([{ name: 'color', type: new Uint32() }])
      const result = resolveMarkdownColors(table, { color: 0xff0000ff })
      expect(result.color).toBe('#ff0000ff')
      expect(result.warnings).toEqual([])
    })

    it('decodes a #rrggbb string color column (alpha defaults to ff)', () => {
      const table = fakeTable([{ name: 'color', type: new Utf8() }])
      const result = resolveMarkdownColors(table, { color: '#00ff00' })
      expect(result.color).toBe('#00ff00ff')
    })

    it('decodes a #rrggbbaa string color column unchanged', () => {
      const table = fakeTable([{ name: 'color', type: new Utf8() }])
      const result = resolveMarkdownColors(table, { color: '#11223344' })
      expect(result.color).toBe('#11223344')
    })

    it('decodes a 4-byte binary color column', () => {
      const table = fakeTable([{ name: 'color', type: new FixedSizeBinary(4) }])
      const result = resolveMarkdownColors(table, { color: new Uint8Array([0x11, 0x22, 0x33, 0x44]) })
      expect(result.color).toBe('#11223344')
    })

    it('treats a null/absent value as no tint', () => {
      const table = fakeTable([{ name: 'color', type: new Uint32() }])
      expect(resolveMarkdownColors(table, {}).color).toBeUndefined()
    })

    it('treats a malformed hex string as no tint', () => {
      const table = fakeTable([{ name: 'color', type: new Utf8() }])
      expect(resolveMarkdownColors(table, { color: 'not-a-color' }).color).toBeUndefined()
    })

    it('warns (without failing) on an unsupported color column type', () => {
      const table = fakeTable([{ name: 'color', type: new Bool() }])
      const result = resolveMarkdownColors(table, { color: true })
      expect(result.color).toBeUndefined()
      expect(result.warnings[0]).toContain("'color' column must be integer")
    })

    it('resolves color and background_color independently', () => {
      const table = fakeTable([
        { name: 'color', type: new Uint32() },
        { name: 'background_color', type: new Utf8() },
      ])
      const result = resolveMarkdownColors(table, { color: 0x000000ff, background_color: '#ffffff' })
      expect(result.color).toBe('#000000ff')
      expect(result.backgroundColor).toBe('#ffffffff')
    })
  })

  describe('rendered DOM (color / background_color)', () => {
    it('sets the inline color on the prose element and switches its classes to text-inherit', () => {
      const table = makeTable({ value: ['Status'], color: ['#ff0000'] })
      const { container } = render(<MarkdownCell {...createMockProps({ content: '$value', data: [table] })} />)
      const prose = container.querySelector('.prose') as HTMLElement
      expect(prose.style.color).not.toBe('')
      expect(prose.className).toContain('prose-headings:text-inherit')
      expect(prose.className).toContain('prose-p:text-inherit')
    })

    it('sets the root background from background_color', () => {
      const table = makeTable({ value: ['Status'], background_color: ['#112233'] })
      const { container } = render(<MarkdownCell {...createMockProps({ content: '$value', data: [table] })} />)
      const root = container.firstElementChild as HTMLElement
      expect(root.style.backgroundColor).not.toBe('')
      expect(root.className).toContain('rounded-sm')
    })

    it('with no color columns, keeps today\'s classes and no inline styles', () => {
      const table = makeTable({ value: ['Status'] })
      const { container } = render(<MarkdownCell {...createMockProps({ content: '$value', data: [table] })} />)
      const root = container.firstElementChild as HTMLElement
      const prose = container.querySelector('.prose') as HTMLElement
      expect(prose.className).toContain('prose-headings:text-theme-text-primary')
      expect(prose.className).toContain('prose-p:text-theme-text-secondary')
      expect(root.getAttribute('style')).toBeNull()
      expect(prose.getAttribute('style')).toBeNull()
    })
  })

  describe('fitFontSize', () => {
    it('returns the largest fitting px for a threshold predicate', () => {
      const result = fitFontSize((px) => px <= 40)
      expect(result).toBe(40)
    })

    it('returns min when nothing fits', () => {
      expect(fitFontSize(() => false)).toBe(MIN_FIT_FONT_PX)
    })

    it('returns max when everything fits', () => {
      expect(fitFontSize(() => true)).toBe(MAX_FIT_FONT_PX)
    })

    it('is monotonic across a range of thresholds', () => {
      for (const threshold of [12, 13, 50, 100, 200, 319, 320]) {
        expect(fitFontSize((px) => px <= threshold)).toBe(threshold)
      }
    })
  })

  describe('editor', () => {
    function createEditorProps(overrides: Partial<CellEditorProps> = {}): CellEditorProps {
      return {
        config: {
          type: 'markdown',
          name: 'Stat',
          layout: { height: 150 },
          content: '',
        } as MarkdownCellConfig,
        onChange: vi.fn(),
        variables: {},
        timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
        cellResults: {},
        cellSelections: {},
        ...overrides,
      }
    }

    it('shows SELECT 1 in the SQL editor when sql is absent', () => {
      render(<markdownMetadata.EditorComponent {...createEditorProps()} />)
      expect(screen.getByDisplayValue('SELECT 1')).toBeInTheDocument()
    })

    it('does not flag a bare-column macro present in availableColumns', () => {
      render(
        <markdownMetadata.EditorComponent
          {...createEditorProps({
            config: { type: 'markdown', name: 'Stat', layout: { height: 150 }, content: '$duration_ms' } as MarkdownCellConfig,
            availableColumns: ['duration_ms'],
          })}
        />
      )
      expect(screen.queryByText(/Unknown variable/)).not.toBeInTheDocument()
    })

    it('toggles Fit to cell, writing options.fit', () => {
      const onChange = vi.fn()
      render(<markdownMetadata.EditorComponent {...createEditorProps({ onChange })} />)
      const checkbox = screen.getByRole('checkbox', { name: 'Fit to cell' })
      expect(checkbox).not.toBeChecked()
      fireEvent.click(checkbox)
      expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ options: { fit: true } }))
    })
  })
})
