import { useEffect, useRef } from 'react'
import type { CellConfig, QueryCellConfig } from './notebook-types'

interface UseCellSortCheckParams {
  executionCells: CellConfig[]
  executeCellByName: (name: string) => void
}

/**
 * Re-executes table/log cells when their sort options change.
 *
 * Tracks sort column and direction for each table/log cell and triggers
 * re-execution when they change. Config is the source of truth for sort state.
 */
export function useCellSortCheck({
  executionCells,
  executeCellByName,
}: UseCellSortCheckParams): void {
  const prevCellOptionsRef = useRef<Map<string, Record<string, unknown>>>(new Map())

  useEffect(() => {
    const checkCell = (cell: CellConfig) => {
      if (cell.type === 'table' || cell.type === 'log') {
        const cellConfig = cell as QueryCellConfig
        const prevOptions = prevCellOptionsRef.current.get(cell.name)
        const currentOptions = cellConfig.options

        const prevSortColumn = prevOptions?.sortColumn
        const prevSortDirection = prevOptions?.sortDirection
        const currentSortColumn = currentOptions?.sortColumn
        const currentSortDirection = currentOptions?.sortDirection

        if (
          prevOptions !== undefined &&
          (prevSortColumn !== currentSortColumn || prevSortDirection !== currentSortDirection)
        ) {
          executeCellByName(cell.name)
        }

        prevCellOptionsRef.current.set(cell.name, currentOptions ?? {})
      }
    }

    executionCells.forEach(checkCell)
  }, [executionCells, executeCellByName])
}
