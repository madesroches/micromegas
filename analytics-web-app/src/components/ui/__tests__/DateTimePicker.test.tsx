import { render, screen, fireEvent } from '@testing-library/react'
import { DateTimePicker } from '../DateTimePicker'

jest.mock('react-day-picker', () => ({
  DayPicker: ({ onSelect, selected }: { onSelect?: (date: Date) => void, selected?: Date }) => (
    <div data-testid="day-picker">
      <button data-testid="day-picker-day" onClick={() => onSelect?.(new Date(2026, 2, 15))}>
        15
      </button>
      {selected && <span data-testid="day-picker-selected">{selected.toISOString()}</span>}
    </div>
  ),
}))

jest.mock('lucide-react', () => ({
  Calendar: () => <span data-testid="calendar-icon">cal</span>,
  Clock: () => <span data-testid="clock-icon">clock</span>,
}))

describe('DateTimePicker', () => {
  const baseDate = new Date(2026, 2, 10, 14, 30)

  it('should open calendar on button click', () => {
    render(<DateTimePicker value={baseDate} onChange={jest.fn()} />)

    expect(screen.queryByTestId('day-picker')).not.toBeInTheDocument()

    const calendarButton = screen.getByRole('button', { name: /mar 10, 2026/i })
    fireEvent.click(calendarButton)

    expect(screen.getByTestId('day-picker')).toBeInTheDocument()
  })

  it('should stay open after date selection', () => {
    const onChange = jest.fn()
    render(<DateTimePicker value={baseDate} onChange={onChange} />)

    const calendarButton = screen.getByRole('button', { name: /mar 10, 2026/i })
    fireEvent.click(calendarButton)

    const dayButton = screen.getByTestId('day-picker-day')
    fireEvent.click(dayButton)

    expect(onChange).toHaveBeenCalled()
    expect(screen.getByTestId('day-picker')).toBeInTheDocument()
  })

  it('should call onChange on time input change', () => {
    const onChange = jest.fn()
    render(<DateTimePicker value={baseDate} onChange={onChange} />)

    const hoursInput = screen.getAllByRole('spinbutton')[0]
    fireEvent.change(hoursInput, { target: { value: '10' } })

    expect(onChange).toHaveBeenCalled()
    const newDate = onChange.mock.calls[0][0] as Date
    expect(newDate.getHours()).toBe(10)
  })

  it('should call onChange for quick action buttons', () => {
    const onChange = jest.fn()
    render(<DateTimePicker value={baseDate} onChange={onChange} />)

    fireEvent.click(screen.getByText('Now'))
    expect(onChange).toHaveBeenCalledTimes(1)

    onChange.mockClear()
    fireEvent.click(screen.getByText('Start of day'))
    expect(onChange).toHaveBeenCalledTimes(1)

    onChange.mockClear()
    fireEvent.click(screen.getByText('End of day'))
    expect(onChange).toHaveBeenCalledTimes(1)
  })

  it('should close calendar on overlay click', () => {
    const { container } = render(<DateTimePicker value={baseDate} onChange={jest.fn()} />)

    const calendarButton = screen.getByRole('button', { name: /mar 10, 2026/i })
    fireEvent.click(calendarButton)
    expect(screen.getByTestId('day-picker')).toBeInTheDocument()

    // The overlay is the fixed inset-0 div
    const overlay = container.querySelector('.fixed.inset-0') as HTMLElement
    fireEvent.click(overlay)

    expect(screen.queryByTestId('day-picker')).not.toBeInTheDocument()
  })

  it('should close calendar on toggle button re-click', () => {
    render(<DateTimePicker value={baseDate} onChange={jest.fn()} />)

    const calendarButton = screen.getByRole('button', { name: /mar 10, 2026/i })
    fireEvent.click(calendarButton)
    expect(screen.getByTestId('day-picker')).toBeInTheDocument()

    fireEvent.click(calendarButton)
    expect(screen.queryByTestId('day-picker')).not.toBeInTheDocument()
  })
})
