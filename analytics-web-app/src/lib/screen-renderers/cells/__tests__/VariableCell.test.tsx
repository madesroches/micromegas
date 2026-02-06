import { render, screen, fireEvent, act } from '@testing-library/react'
import { VariableTitleBarContent, variableMetadata } from '../VariableCell'
import { CellRendererProps } from '../../cell-registry'
import type { VariableCellConfig, CellState } from '../../notebook-types'

// Use fake timers for debounce testing
beforeEach(() => {
  jest.useFakeTimers()
})

afterEach(() => {
  jest.useRealTimers()
})

// Create a minimal mock for required CellRendererProps
const createMockProps = (overrides: Partial<CellRendererProps> = {}): CellRendererProps => ({
  name: 'test-variable',
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

describe('VariableTitleBarContent', () => {
  describe('loading state', () => {
    it('should show compact loading indicator when status is loading', () => {
      render(<VariableTitleBarContent {...createMockProps({ status: 'loading' })} />)
      expect(screen.getByText('Loading...')).toBeInTheDocument()
    })

    it('should not show inputs when loading', () => {
      render(<VariableTitleBarContent {...createMockProps({ status: 'loading' })} />)
      expect(screen.queryByRole('combobox')).not.toBeInTheDocument()
      expect(screen.queryByRole('textbox')).not.toBeInTheDocument()
    })
  })

  describe('combobox type', () => {
    it('should render select element for combobox type', () => {
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'combobox',
            variableOptions: [
              { label: 'Option 1', value: 'opt1' },
              { label: 'Option 2', value: 'opt2' },
            ],
          })}
        />
      )
      expect(screen.getByRole('combobox')).toBeInTheDocument()
    })

    it('should render all options', () => {
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'combobox',
            variableOptions: [
              { label: 'Option 1', value: 'opt1' },
              { label: 'Option 2', value: 'opt2' },
              { label: 'Option 3', value: 'opt3' },
            ],
          })}
        />
      )
      expect(screen.getByText('Option 1')).toBeInTheDocument()
      expect(screen.getByText('Option 2')).toBeInTheDocument()
      expect(screen.getByText('Option 3')).toBeInTheDocument()
    })

    it('should show "No options available" when options are empty', () => {
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'combobox',
            variableOptions: [],
          })}
        />
      )
      expect(screen.getByText('No options available')).toBeInTheDocument()
    })

    it('should show "No options available" when options are undefined', () => {
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'combobox',
            variableOptions: undefined,
          })}
        />
      )
      expect(screen.getByText('No options available')).toBeInTheDocument()
    })

    it('should select the current value', () => {
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'combobox',
            value: 'opt2',
            variableOptions: [
              { label: 'Option 1', value: 'opt1' },
              { label: 'Option 2', value: 'opt2' },
            ],
          })}
        />
      )
      const select = screen.getByRole('combobox') as HTMLSelectElement
      expect(select.value).toBe('opt2')
    })

    it('should call onValueChange when selection changes', () => {
      const onValueChange = jest.fn()
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'combobox',
            value: 'opt1',
            onValueChange,
            variableOptions: [
              { label: 'Option 1', value: 'opt1' },
              { label: 'Option 2', value: 'opt2' },
            ],
          })}
        />
      )

      const select = screen.getByRole('combobox')
      fireEvent.change(select, { target: { value: 'opt2' } })

      expect(onValueChange).toHaveBeenCalledWith('opt2')
    })
  })

  describe('text type', () => {
    it('should render text input for text type', () => {
      render(<VariableTitleBarContent {...createMockProps({ variableType: 'text' })} />)
      expect(screen.getByRole('textbox')).toBeInTheDocument()
    })

    it('should display current value', () => {
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'text',
            value: 'my text value',
          })}
        />
      )
      const input = screen.getByRole('textbox') as HTMLInputElement
      expect(input.value).toBe('my text value')
    })

    it('should handle empty value as empty string', () => {
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'text',
            value: undefined,
          })}
        />
      )
      const input = screen.getByRole('textbox') as HTMLInputElement
      expect(input.value).toBe('')
    })

    it('should call onValueChange when text changes (debounced)', () => {
      const onValueChange = jest.fn()
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'text',
            onValueChange,
          })}
        />
      )

      const input = screen.getByRole('textbox')
      fireEvent.change(input, { target: { value: 'new value' } })

      expect(onValueChange).not.toHaveBeenCalled()

      act(() => {
        jest.advanceTimersByTime(300)
      })

      expect(onValueChange).toHaveBeenCalledWith('new value')
    })
  })

  describe('expression type', () => {
    it('should render read-only computed value for expression type', () => {
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'expression',
            value: '5s',
          })}
        />
      )
      expect(screen.getByText('5s')).toBeInTheDocument()
    })

    it('should show placeholder when expression has not been computed', () => {
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'expression',
            value: undefined,
          })}
        />
      )
      expect(screen.getByText('(not yet computed)')).toBeInTheDocument()
    })
  })

  describe('variableMetadata.execute (expression)', () => {
    it('should evaluate expression and return result', async () => {
      Object.defineProperty(window, 'innerWidth', { value: 1920, writable: true })
      Object.defineProperty(window, 'devicePixelRatio', { value: 2, writable: true })

      const config: VariableCellConfig = {
        type: 'variable',
        name: 'bin',
        variableType: 'expression',
        expression: 'snap_interval($duration_ms / $innerWidth)',
        layout: { height: 0 },
      }
      const result = await variableMetadata.execute!(config, {
        variables: {},
        timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
        runQuery: jest.fn(),
      })
      // 86400000 / 1920 = 45000 -> snaps to 30s
      expect(result).toEqual({ data: null, expressionResult: '30s' })
    })

    it('should return null when expression is empty', async () => {
      const config: VariableCellConfig = {
        type: 'variable',
        name: 'bin',
        variableType: 'expression',
        expression: '',
        layout: { height: 0 },
      }
      const result = await variableMetadata.execute!(config, {
        variables: {},
        timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
        runQuery: jest.fn(),
      })
      expect(result).toBeNull()
    })

    it('should pass upstream variables to expression context', async () => {
      Object.defineProperty(window, 'innerWidth', { value: 1920, writable: true })
      Object.defineProperty(window, 'devicePixelRatio', { value: 1, writable: true })

      const config: VariableCellConfig = {
        type: 'variable',
        name: 'result',
        variableType: 'expression',
        expression: '$myVar',
        layout: { height: 0 },
      }
      const result = await variableMetadata.execute!(config, {
        variables: { myVar: 'hello' },
        timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
        runQuery: jest.fn(),
      })
      expect(result).toEqual({ data: null, expressionResult: 'hello' })
    })
  })

  describe('variableMetadata.onExecutionComplete (expression)', () => {
    it('should set variable value from expression result', () => {
      const setVariableValue = jest.fn()
      const config: VariableCellConfig = {
        type: 'variable',
        name: 'bin',
        variableType: 'expression',
        layout: { height: 0 },
      }
      const state: CellState = { status: 'success', data: null, expressionResult: '30s' }
      variableMetadata.onExecutionComplete!(config, state, {
        setVariableValue,
        currentValue: undefined,
      })
      expect(setVariableValue).toHaveBeenCalledWith('bin', '30s')
    })

    it('should not call setVariableValue when expressionResult is undefined', () => {
      const setVariableValue = jest.fn()
      const config: VariableCellConfig = {
        type: 'variable',
        name: 'bin',
        variableType: 'expression',
        layout: { height: 0 },
      }
      const state: CellState = { status: 'success', data: null }
      variableMetadata.onExecutionComplete!(config, state, {
        setVariableValue,
        currentValue: undefined,
      })
      expect(setVariableValue).not.toHaveBeenCalled()
    })
  })

  describe('default behavior', () => {
    it('should default to text type when variableType is undefined', () => {
      render(<VariableTitleBarContent {...createMockProps({ variableType: undefined })} />)
      expect(screen.getByRole('textbox')).toBeInTheDocument()
    })

    it('should handle missing onValueChange gracefully', () => {
      render(
        <VariableTitleBarContent
          {...createMockProps({
            variableType: 'text',
            onValueChange: undefined,
          })}
        />
      )

      const input = screen.getByRole('textbox')
      expect(() => {
        fireEvent.change(input, { target: { value: 'test' } })
      }).not.toThrow()
    })
  })
})
