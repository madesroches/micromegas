import { render, screen, fireEvent } from '@testing-library/react'
import { vectorFromArray, Table, Timestamp, TimeUnit } from 'apache-arrow'
import { AvailableVariablesPanel } from '../AvailableVariablesPanel'

describe('AvailableVariablesPanel', () => {
  const defaultProps = {
    variables: {} as Record<string, string>,
    timeRange: { begin: '2024-01-01T00:00:00Z', end: '2024-01-02T00:00:00Z' },
    cellResults: {} as Record<string, Table>,
    cellSelections: {} as Record<string, Record<string, unknown>>,
  }

  it('should render time range variables', () => {
    render(<AvailableVariablesPanel {...defaultProps} />)
    expect(screen.getByText('$from')).toBeInTheDocument()
    expect(screen.getByText('$to')).toBeInTheDocument()
  })

  it('should display cell selection with formatted timestamp', () => {
    const timestampType = new Timestamp(TimeUnit.MILLISECOND, null)
    // 2024-01-15T10:30:00.000Z in milliseconds
    const ms = 1705314600000
    const vector = vectorFromArray([ms], timestampType)
    const table = new Table({ frame_begin: vector, name: vector })

    render(
      <AvailableVariablesPanel
        {...defaultProps}
        cellResults={{ upstream: table }}
        cellSelections={{ upstream: { frame_begin: ms, name: 'test' } }}
      />
    )

    // Expand the selection entry
    fireEvent.click(screen.getByText(/upstream\.selected/))

    // The timestamp value should be formatted as ISO, not raw ms
    expect(screen.getByText('2024-01-15T10:30:00.000Z')).toBeInTheDocument()
    // Non-timestamp values should still show as raw strings
    expect(screen.getByText('test')).toBeInTheDocument()
  })

  it('should display raw value when cellResults has no type info', () => {
    const ms = 1705314600000

    render(
      <AvailableVariablesPanel
        {...defaultProps}
        cellSelections={{ upstream: { frame_begin: ms } }}
      />
    )

    // Expand the selection entry
    fireEvent.click(screen.getByText(/upstream\.selected/))

    // Without type info, should fall back to String()
    expect(screen.getByText(String(ms))).toBeInTheDocument()
  })

  it('should display dash for null values in selection', () => {
    render(
      <AvailableVariablesPanel
        {...defaultProps}
        cellSelections={{ upstream: { col: null } }}
      />
    )

    fireEvent.click(screen.getByText(/upstream\.selected/))
    expect(screen.getByText('-')).toBeInTheDocument()
  })
})
