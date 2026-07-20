import { render, screen, fireEvent } from '@testing-library/react'
import { CellTimeRangeField } from '../CellTimeRangeField'

describe('CellTimeRangeField', () => {
  it('renders empty inputs when value is unset', () => {
    render(<CellTimeRangeField value={undefined} onChange={jest.fn()} />)
    expect(screen.getByPlaceholderText(/\$from/)).toHaveValue('')
    expect(screen.getByPlaceholderText(/\$to/)).toHaveValue('')
  })

  it('renders existing from/to values', () => {
    render(<CellTimeRangeField value={{ from: 'now-1h', to: 'now' }} onChange={jest.fn()} />)
    expect(screen.getByPlaceholderText(/\$from/)).toHaveValue('now-1h')
    expect(screen.getByPlaceholderText(/\$to/)).toHaveValue('now')
  })

  it('emits the full {from,to} object when editing "from" with an existing "to"', () => {
    const onChange = jest.fn()
    render(<CellTimeRangeField value={{ from: '', to: 'now' }} onChange={onChange} />)
    fireEvent.change(screen.getByPlaceholderText(/\$from/), { target: { value: 'now-1h' } })
    expect(onChange).toHaveBeenCalledWith({ from: 'now-1h', to: 'now' })
  })

  it('emits the full {from,to} object when editing "to" with an existing "from"', () => {
    const onChange = jest.fn()
    render(<CellTimeRangeField value={{ from: 'now-1h', to: '' }} onChange={onChange} />)
    fireEvent.change(screen.getByPlaceholderText(/\$to/), { target: { value: 'now' } })
    expect(onChange).toHaveBeenCalledWith({ from: 'now-1h', to: 'now' })
  })

  it('emits undefined when both bounds are cleared', () => {
    const onChange = jest.fn()
    render(<CellTimeRangeField value={{ from: 'now-1h', to: '' }} onChange={onChange} />)
    fireEvent.change(screen.getByPlaceholderText(/\$from/), { target: { value: '' } })
    expect(onChange).toHaveBeenCalledWith(undefined)
  })
})
