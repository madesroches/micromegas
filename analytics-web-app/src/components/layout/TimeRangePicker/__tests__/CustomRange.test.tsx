import { render, screen, fireEvent } from '@testing-library/react'
import { CustomRange } from '../CustomRange'

jest.mock('@/components/ui/DateTimePicker', () => ({
  DateTimePicker: ({ value, onChange }: { value?: Date, onChange: (d: Date) => void }) => (
    <div data-testid="date-time-picker">
      <button data-testid="mock-date-select" onClick={() => onChange(new Date(2026, 2, 15))}>
        Select Date
      </button>
      {value && <span data-testid="picker-value">{value.toISOString()}</span>}
    </div>
  ),
}))

jest.mock('lucide-react', () => ({
  Calendar: () => <span data-testid="calendar-icon">cal</span>,
}))

describe('CustomRange', () => {
  const defaultProps = {
    from: 'now-1h',
    to: 'now',
    onApply: jest.fn(),
  }

  beforeEach(() => {
    jest.clearAllMocks()
  })

  it('should show calendar when from toggle is clicked', () => {
    render(<CustomRange {...defaultProps} />)

    expect(screen.queryByTestId('date-time-picker')).not.toBeInTheDocument()

    fireEvent.click(screen.getByLabelText('Open start calendar'))

    expect(screen.getByTestId('date-time-picker')).toBeInTheDocument()
  })

  it('should keep from calendar visible after date selection', () => {
    render(<CustomRange {...defaultProps} />)

    fireEvent.click(screen.getByLabelText('Open start calendar'))
    expect(screen.getByTestId('date-time-picker')).toBeInTheDocument()

    fireEvent.click(screen.getByTestId('mock-date-select'))

    // Calendar should still be visible
    expect(screen.getByTestId('date-time-picker')).toBeInTheDocument()
  })

  it('should keep to calendar visible after date selection', () => {
    render(<CustomRange {...defaultProps} />)

    fireEvent.click(screen.getByLabelText('Open end calendar'))
    expect(screen.getByTestId('date-time-picker')).toBeInTheDocument()

    fireEvent.click(screen.getByTestId('mock-date-select'))

    // Calendar should still be visible
    expect(screen.getByTestId('date-time-picker')).toBeInTheDocument()
  })

  it('should close calendar when toggle button clicked again', () => {
    render(<CustomRange {...defaultProps} />)

    const toggle = screen.getByLabelText('Open start calendar')
    fireEvent.click(toggle)
    expect(screen.getByTestId('date-time-picker')).toBeInTheDocument()

    fireEvent.click(toggle)
    expect(screen.queryByTestId('date-time-picker')).not.toBeInTheDocument()
  })

  it('should update the from input after date selection', () => {
    render(<CustomRange {...defaultProps} />)

    fireEvent.click(screen.getByLabelText('Open start calendar'))
    fireEvent.click(screen.getByTestId('mock-date-select'))

    // The from input should have been updated from the relative expression
    const fromInput = screen.getByPlaceholderText('now-1h or ISO date') as HTMLInputElement
    expect(fromInput.value).not.toBe('now-1h')
  })
})
