import { renderHook } from '@testing-library/react'
import { useCellSortCheck } from '../useCellSortCheck'
import type { CellConfig } from '../notebook-types'

function cell(sortColumn?: string): CellConfig {
  return {
    type: 'table',
    name: 'T',
    sql: 'SELECT 1',
    layout: { height: 'auto' },
    options: { sortColumn },
  } as unknown as CellConfig
}

describe('useCellSortCheck admin mode', () => {
  it('re-runs the cell on a sort change when off', () => {
    const execute = vi.fn()
    const { rerender } = renderHook(
      ({ cells }) => useCellSortCheck({ executionCells: cells, executeCellByName: execute }),
      { initialProps: { cells: [cell('a')] } }
    )
    rerender({ cells: [cell('b')] })
    expect(execute).toHaveBeenCalledWith('T')
  })

  it('does not re-run the cell on a sort change when on', () => {
    const execute = vi.fn()
    const { rerender } = renderHook(
      ({ cells }) =>
        useCellSortCheck({ executionCells: cells, executeCellByName: execute, adminMode: true }),
      { initialProps: { cells: [cell('a')] } }
    )
    rerender({ cells: [cell('b')] })
    expect(execute).not.toHaveBeenCalled()
  })
})
