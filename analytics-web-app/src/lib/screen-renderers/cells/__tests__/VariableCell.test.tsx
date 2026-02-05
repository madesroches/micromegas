import { render, screen, fireEvent, act } from '@testing-library/react'
import { VariableTitleBarContent } from '../VariableCell'
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
