import { useCallback, useEffect, useMemo, useRef } from 'react'
import type { CellConfig, HorizontalGroupCellConfig } from './notebook-types'

type NavTarget = { cellIndex: number; childName: string | null }

interface UseNotebookKeyboardNavParams {
  cells: CellConfig[]
  selectedCellIndex: number | null
  selectedChildName: string | null
  setSelectedCellIndex: (index: number | null) => void
  setSelectedChildName: (name: string | null) => void
  disabled: boolean
}

interface UseNotebookKeyboardNavResult {
  /** Callback to register a cell's DOM element by name */
  setCellRef: (name: string, element: HTMLElement | null) => void
}

export function useNotebookKeyboardNav({
  cells,
  selectedCellIndex,
  selectedChildName,
  setSelectedCellIndex,
  setSelectedChildName,
  disabled,
}: UseNotebookKeyboardNavParams): UseNotebookKeyboardNavResult {
  const refMap = useRef(new Map<string, HTMLElement>())

  const setCellRef = useCallback((name: string, element: HTMLElement | null) => {
    if (element) {
      refMap.current.set(name, element)
    } else {
      refMap.current.delete(name)
    }
  }, [])

  const navTargets = useMemo<NavTarget[]>(
    () =>
      cells.flatMap<NavTarget>((cell, i) =>
        cell.type === 'hg'
          ? cell.layout.collapsed
            ? []
            : (cell as HorizontalGroupCellConfig).children.map((child) => ({
                cellIndex: i,
                childName: child.name,
              }))
          : [{ cellIndex: i, childName: null }],
      ),
    [cells],
  )

  // Find current position in navTargets, handling the HG-group-selected case
  const findCurrentIndex = useCallback((): number => {
    if (selectedCellIndex === null) return -1

    // Direct match
    const exact = navTargets.findIndex(
      (t) => t.cellIndex === selectedCellIndex && t.childName === selectedChildName,
    )
    if (exact !== -1) return exact

    // HG group header selected (not a navTarget itself) — resolve to first
    // navTarget at or after the selected cellIndex
    const nearest = navTargets.findIndex((t) => t.cellIndex >= selectedCellIndex)
    return nearest
  }, [navTargets, selectedCellIndex, selectedChildName])

  // Keydown handler
  useEffect(() => {
    if (disabled) return

    const handler = (e: KeyboardEvent) => {
      if (!e.altKey) return
      if (e.key !== 'PageDown' && e.key !== 'PageUp') return

      e.preventDefault()

      if (navTargets.length === 0) return

      const currentIndex = findCurrentIndex()
      let newIndex: number

      if (e.key === 'PageDown') {
        if (currentIndex === -1) {
          newIndex = 0
        } else if (currentIndex < navTargets.length - 1) {
          newIndex = currentIndex + 1
        } else {
          return // at boundary
        }
      } else {
        // PageUp
        if (currentIndex === -1) {
          newIndex = navTargets.length - 1
        } else if (currentIndex > 0) {
          newIndex = currentIndex - 1
        } else {
          return // at boundary
        }
      }

      const target = navTargets[newIndex]
      setSelectedCellIndex(target.cellIndex)
      setSelectedChildName(target.childName)
    }

    document.addEventListener('keydown', handler)
    return () => document.removeEventListener('keydown', handler)
  }, [disabled, navTargets, findCurrentIndex, setSelectedCellIndex, setSelectedChildName])

  // Scroll into view on any selection change (benefits mouse selection too)
  useEffect(() => {
    if (selectedCellIndex === null) return
    const name =
      selectedChildName ?? cells[selectedCellIndex]?.name
    if (!name) return
    const el = refMap.current.get(name)
    el?.scrollIntoView?.({ behavior: 'smooth', block: 'nearest' })
  }, [selectedCellIndex, selectedChildName, cells])

  return { setCellRef }
}
