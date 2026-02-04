import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { PerfettoExportCell, perfettoExportMetadata } from '../PerfettoExportCell'
import type { CellRendererProps } from '../../cell-registry'
import { generateTrace } from '@/lib/api'
import { openInPerfetto } from '@/lib/perfetto'

// Mock the API and Perfetto modules to prevent actual calls
jest.mock('@/lib/api', () => ({
  generateTrace: jest.fn(),
}))

jest.mock('@/lib/perfetto', () => ({
  openInPerfetto: jest.fn(),
}))

const mockGenerateTrace = generateTrace as jest.MockedFunction<typeof generateTrace>
const mockOpenInPerfetto = openInPerfetto as jest.MockedFunction<typeof openInPerfetto>

// Create minimal mock props for CellRendererProps
const createMockProps = (overrides: Partial<CellRendererProps> = {}): CellRendererProps => ({
  name: 'test-perfetto-export',
  sql: undefined,
  options: {
    processIdVar: '$process_id',
    spanType: 'both',
  },
  data: null,
  status: 'success',
  error: undefined,
  timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
  variables: {},
  isEditing: false,
  onRun: jest.fn(),
  onSqlChange: jest.fn(),
  onOptionsChange: jest.fn(),
  ...overrides,
})

describe('PerfettoExportCell', () => {
  describe('display states', () => {
    describe('warning when variable not found', () => {
      it('should show warning when process_id variable is missing', () => {
        render(<PerfettoExportCell {...createMockProps({ variables: {} })} />)
        expect(screen.getByText(/Variable "\$process_id" not found/)).toBeInTheDocument()
      })

      it('should show warning with custom variable name when not found', () => {
        render(
          <PerfettoExportCell
            {...createMockProps({
              options: { processIdVar: '$my_process', spanType: 'both' },
              variables: {},
            })}
          />
        )
        expect(screen.getByText(/Variable "\$my_process" not found/)).toBeInTheDocument()
      })

      it('should not show warning when variable exists', () => {
        render(
          <PerfettoExportCell
            {...createMockProps({
              variables: { process_id: 'abc-123' },
            })}
          />
        )
        expect(screen.queryByText(/Variable .* not found/)).not.toBeInTheDocument()
      })

      it('should show warning when variable exists but is empty string', () => {
        render(
          <PerfettoExportCell
            {...createMockProps({
              variables: { process_id: '' },
            })}
          />
        )
        expect(screen.getByText(/Variable "\$process_id" is empty/)).toBeInTheDocument()
      })
    })

    describe('button states', () => {
      it('should disable button when process_id variable is missing', () => {
        render(<PerfettoExportCell {...createMockProps({ variables: {} })} />)
        const button = screen.getByRole('button', { name: /Open in Perfetto/i })
        expect(button).toBeDisabled()
      })

      it('should enable button when process_id variable exists', () => {
        render(
          <PerfettoExportCell
            {...createMockProps({
              variables: { process_id: 'abc-123' },
            })}
          />
        )
        const button = screen.getByRole('button', { name: /Open in Perfetto/i })
        expect(button).not.toBeDisabled()
      })
    })

    describe('span type info display', () => {
      it('should show "Thread + Async spans" for spanType both', () => {
        render(
          <PerfettoExportCell
            {...createMockProps({
              options: { processIdVar: '$process_id', spanType: 'both' },
              variables: { process_id: 'abc-123' },
            })}
          />
        )
        expect(screen.getByText('Thread + Async spans')).toBeInTheDocument()
      })

      it('should show "Thread spans only" for spanType thread', () => {
        render(
          <PerfettoExportCell
            {...createMockProps({
              options: { processIdVar: '$process_id', spanType: 'thread' },
              variables: { process_id: 'abc-123' },
            })}
          />
        )
        expect(screen.getByText('Thread spans only')).toBeInTheDocument()
      })

      it('should show "Async spans only" for spanType async', () => {
        render(
          <PerfettoExportCell
            {...createMockProps({
              options: { processIdVar: '$process_id', spanType: 'async' },
              variables: { process_id: 'abc-123' },
            })}
          />
        )
        expect(screen.getByText('Async spans only')).toBeInTheDocument()
      })

      it('should not show span type info when variable is missing', () => {
        render(<PerfettoExportCell {...createMockProps({ variables: {} })} />)
        expect(screen.queryByText(/spans/i)).not.toBeInTheDocument()
      })
    })
  })

  describe('variable resolution', () => {
    it('should resolve variable without $ prefix in options', () => {
      render(
        <PerfettoExportCell
          {...createMockProps({
            options: { processIdVar: 'process_id', spanType: 'both' },
            variables: { process_id: 'abc-123' },
          })}
        />
      )
      // If variable resolved, button should be enabled
      const button = screen.getByRole('button', { name: /Open in Perfetto/i })
      expect(button).not.toBeDisabled()
    })

    it('should resolve variable with $ prefix in options', () => {
      render(
        <PerfettoExportCell
          {...createMockProps({
            options: { processIdVar: '$process_id', spanType: 'both' },
            variables: { process_id: 'abc-123' },
          })}
        />
      )
      const button = screen.getByRole('button', { name: /Open in Perfetto/i })
      expect(button).not.toBeDisabled()
    })

    it('should use default $process_id when processIdVar is not set', () => {
      render(
        <PerfettoExportCell
          {...createMockProps({
            options: { spanType: 'both' },
            variables: { process_id: 'abc-123' },
          })}
        />
      )
      const button = screen.getByRole('button', { name: /Open in Perfetto/i })
      expect(button).not.toBeDisabled()
    })

    it('should handle multi-column variable values', () => {
      render(
        <PerfettoExportCell
          {...createMockProps({
            variables: { process_id: { id: 'abc-123', name: 'test' } },
          })}
        />
      )
      // Multi-column values are converted to JSON string via getVariableString
      // Button should be enabled since we have a non-empty value
      const button = screen.getByRole('button', { name: /Open in Perfetto/i })
      expect(button).not.toBeDisabled()
    })

    it('should handle custom variable names', () => {
      render(
        <PerfettoExportCell
          {...createMockProps({
            options: { processIdVar: '$custom_var', spanType: 'both' },
            variables: { custom_var: 'custom-value' },
          })}
        />
      )
      const button = screen.getByRole('button', { name: /Open in Perfetto/i })
      expect(button).not.toBeDisabled()
      expect(screen.queryByText(/Variable .* not found/)).not.toBeInTheDocument()
    })
  })

  describe('trace generation and caching', () => {
    const mockBuffer = new ArrayBuffer(100)

    beforeEach(() => {
      jest.clearAllMocks()
      mockGenerateTrace.mockResolvedValue(mockBuffer)
      mockOpenInPerfetto.mockResolvedValue(undefined)
    })

    it('should call generateTrace when clicking Open in Perfetto', async () => {
      render(
        <PerfettoExportCell
          {...createMockProps({
            variables: { process_id: 'abc-123' },
            timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
          })}
        />
      )

      fireEvent.click(screen.getByRole('button', { name: /Open in Perfetto/i }))

      await waitFor(() => {
        expect(mockGenerateTrace).toHaveBeenCalledTimes(1)
        expect(mockGenerateTrace).toHaveBeenCalledWith(
          'abc-123',
          expect.objectContaining({
            include_thread_spans: true,
            include_async_spans: true,
          }),
          expect.any(Function),
          { returnBuffer: true }
        )
      })
    })

    it('should use cached buffer on second click (no re-fetch)', async () => {
      render(
        <PerfettoExportCell
          {...createMockProps({
            variables: { process_id: 'abc-123' },
            timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
          })}
        />
      )

      const button = screen.getByRole('button', { name: /Open in Perfetto/i })

      // First click - should fetch
      fireEvent.click(button)
      await waitFor(() => expect(mockGenerateTrace).toHaveBeenCalledTimes(1))
      await waitFor(() => expect(mockOpenInPerfetto).toHaveBeenCalledTimes(1))

      // Second click - should use cache
      fireEvent.click(button)
      await waitFor(() => expect(mockOpenInPerfetto).toHaveBeenCalledTimes(2))

      // generateTrace should still only be called once (cached)
      expect(mockGenerateTrace).toHaveBeenCalledTimes(1)
    })

    it('should show error message when generateTrace fails', async () => {
      mockGenerateTrace.mockRejectedValue(new Error('Network error'))

      render(
        <PerfettoExportCell
          {...createMockProps({
            variables: { process_id: 'abc-123' },
          })}
        />
      )

      fireEvent.click(screen.getByRole('button', { name: /Open in Perfetto/i }))

      await waitFor(() => {
        expect(screen.getByText('Network error')).toBeInTheDocument()
      })
      expect(screen.getByRole('button', { name: /Retry/i })).toBeInTheDocument()
      expect(screen.getByRole('button', { name: /Dismiss/i })).toBeInTheDocument()
    })

    it('should show "Download Instead" button when openInPerfetto fails but buffer exists', async () => {
      mockOpenInPerfetto.mockRejectedValue({ type: 'popup_blocked', message: 'Popup blocked' })

      render(
        <PerfettoExportCell
          {...createMockProps({
            variables: { process_id: 'abc-123' },
          })}
        />
      )

      fireEvent.click(screen.getByRole('button', { name: /Open in Perfetto/i }))

      await waitFor(() => {
        expect(screen.getByText('Popup blocked')).toBeInTheDocument()
      })
      expect(screen.getByRole('button', { name: /Download Instead/i })).toBeInTheDocument()
    })

    it('should dismiss error when clicking Dismiss button', async () => {
      mockGenerateTrace.mockRejectedValue(new Error('Test error'))

      render(
        <PerfettoExportCell
          {...createMockProps({
            variables: { process_id: 'abc-123' },
          })}
        />
      )

      fireEvent.click(screen.getByRole('button', { name: /Open in Perfetto/i }))
      await waitFor(() => expect(screen.getByText('Test error')).toBeInTheDocument())

      fireEvent.click(screen.getByRole('button', { name: /Dismiss/i }))

      expect(screen.queryByText('Test error')).not.toBeInTheDocument()
    })
  })
})

describe('perfettoExportMetadata', () => {
  describe('static properties', () => {
    it('should have correct label', () => {
      expect(perfettoExportMetadata.label).toBe('Perfetto Export')
    })

    it('should have correct icon', () => {
      expect(perfettoExportMetadata.icon).toBe('E')
    })

    it('should have correct description', () => {
      expect(perfettoExportMetadata.description).toBe('Export spans to Perfetto trace viewer')
    })

    it('should show type badge', () => {
      expect(perfettoExportMetadata.showTypeBadge).toBe(true)
    })

    it('should have default height of 80', () => {
      expect(perfettoExportMetadata.defaultHeight).toBe(80)
    })

    it('should not block downstream cells', () => {
      expect(perfettoExportMetadata.canBlockDownstream).toBe(false)
    })

    it('should not have an execute method (user-triggered action)', () => {
      expect(perfettoExportMetadata.execute).toBeUndefined()
    })
  })

  describe('createDefaultConfig', () => {
    it('should return correct default configuration', () => {
      const config = perfettoExportMetadata.createDefaultConfig()
      expect(config).toEqual({
        type: 'perfettoexport',
        processIdVar: '$process_id',
        spanType: 'both',
      })
    })
  })

  describe('getRendererProps', () => {
    it('should extract options from config', () => {
      const config = {
        name: 'test',
        type: 'perfettoexport' as const,
        layout: { height: 80 },
        processIdVar: '$my_var',
        spanType: 'thread' as const,
      }
      const state = { status: 'success' as const, data: null }

      const props = perfettoExportMetadata.getRendererProps(config, state)

      expect(props).toEqual({
        status: 'success',
        options: {
          processIdVar: '$my_var',
          spanType: 'thread',
        },
      })
    })

    it('should use defaults when config values are undefined', () => {
      const config = {
        name: 'test',
        type: 'perfettoexport' as const,
        layout: { height: 80 },
      }
      const state = { status: 'idle' as const, data: null }

      const props = perfettoExportMetadata.getRendererProps(config, state)

      expect(props).toEqual({
        status: 'idle',
        options: {
          processIdVar: '$process_id',
          spanType: 'both',
        },
      })
    })
  })
})
