import { render, screen, fireEvent, act } from '@testing-library/react'
import { VariableCell, VariableTitleBarContent } from '../VariableCell'
import { CellRendererProps } from '../../cell-registry'

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

describe('VariableCell', () => {
  describe('loading state', () => {
    it('should show loading spinner when status is loading', () => {
      render(<VariableCell {...createMockProps({ status: 'loading' })} />)
      expect(screen.getByText('Loading options...')).toBeInTheDocument()
    })

    it('should not show inputs when loading', () => {
      render(<VariableCell {...createMockProps({ status: 'loading' })} />)
      expect(screen.queryByRole('combobox')).not.toBeInTheDocument()
      expect(screen.queryByRole('textbox')).not.toBeInTheDocument()
      expect(screen.queryByRole('spinbutton')).not.toBeInTheDocument()
    })
  })

  describe('combobox type', () => {
    it('should render select element for combobox type', () => {
      render(
        <VariableCell
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
        <VariableCell
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
        <VariableCell
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
        <VariableCell
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
        <VariableCell
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
        <VariableCell
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
      render(<VariableCell {...createMockProps({ variableType: 'text' })} />)
      expect(screen.getByRole('textbox')).toBeInTheDocument()
    })

    it('should show placeholder text', () => {
      render(<VariableCell {...createMockProps({ variableType: 'text' })} />)
      expect(screen.getByPlaceholderText('Enter value...')).toBeInTheDocument()
    })

    it('should display current value', () => {
      render(
        <VariableCell
          {...createMockProps({
            variableType: 'text',
            value: 'my text value',
          })}
        />
      )
      const input = screen.getByRole('textbox') as HTMLInputElement
      expect(input.value).toBe('my text value')
    })

    it('should call onValueChange when text changes (debounced)', () => {
      const onValueChange = jest.fn()
      render(
        <VariableCell
          {...createMockProps({
            variableType: 'text',
            onValueChange,
          })}
        />
      )

      const input = screen.getByRole('textbox')
      fireEvent.change(input, { target: { value: 'new value' } })

      // Value change is debounced, so it shouldn't be called immediately
      expect(onValueChange).not.toHaveBeenCalled()

      // Advance timers by debounce delay (300ms)
      act(() => {
        jest.advanceTimersByTime(300)
      })

      expect(onValueChange).toHaveBeenCalledWith('new value')
    })

    it('should handle empty value as empty string', () => {
      render(
        <VariableCell
          {...createMockProps({
            variableType: 'text',
            value: undefined,
          })}
        />
      )
      const input = screen.getByRole('textbox') as HTMLInputElement
      expect(input.value).toBe('')
    })
  })

  describe('number type', () => {
    it('should render number input for number type', () => {
      render(<VariableCell {...createMockProps({ variableType: 'number' })} />)
      expect(screen.getByRole('spinbutton')).toBeInTheDocument()
    })

    it('should show placeholder', () => {
      render(<VariableCell {...createMockProps({ variableType: 'number' })} />)
      expect(screen.getByPlaceholderText('0')).toBeInTheDocument()
    })

    it('should display current numeric value', () => {
      render(
        <VariableCell
          {...createMockProps({
            variableType: 'number',
            value: '42',
          })}
        />
      )
      const input = screen.getByRole('spinbutton') as HTMLInputElement
      expect(input.value).toBe('42')
    })

    it('should call onValueChange when number changes (debounced)', () => {
      const onValueChange = jest.fn()
      render(
        <VariableCell
          {...createMockProps({
            variableType: 'number',
            onValueChange,
          })}
        />
      )

      const input = screen.getByRole('spinbutton')
      fireEvent.change(input, { target: { value: '100' } })

      // Value change is debounced, so it shouldn't be called immediately
      expect(onValueChange).not.toHaveBeenCalled()

      // Advance timers by debounce delay (300ms)
      act(() => {
        jest.advanceTimersByTime(300)
      })

      expect(onValueChange).toHaveBeenCalledWith('100')
    })
  })

  describe('default behavior', () => {
    it('should default to text type when variableType is undefined', () => {
      render(<VariableCell {...createMockProps({ variableType: undefined })} />)
      // Should render text input (textbox role)
      expect(screen.getByRole('textbox')).toBeInTheDocument()
    })

    it('should handle missing onValueChange gracefully', () => {
      render(
        <VariableCell
          {...createMockProps({
            variableType: 'text',
            onValueChange: undefined,
          })}
        />
      )

      const input = screen.getByRole('textbox')
      // Should not throw when changing value
      expect(() => {
        fireEvent.change(input, { target: { value: 'test' } })
      }).not.toThrow()
    })
  })
})

describe('VariableTitleBarContent', () => {
  describe('loading state', () => {
    it('should show compact loading indicator when status is loading', () => {
      render(<VariableTitleBarContent {...createMockProps({ status: 'loading' })} />)
      expect(screen.getByText('Loading...')).toBeInTheDocument()
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

  describe('number type', () => {
    it('should render number input for number type', () => {
      render(<VariableTitleBarContent {...createMockProps({ variableType: 'number' })} />)
      expect(screen.getByRole('spinbutton')).toBeInTheDocument()
    })
  })
})
