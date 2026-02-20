import { useState, useCallback, useMemo, useRef } from 'react'
import {
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  DragEndEvent,
  DragStartEvent,
  DragOverEvent,
} from '@dnd-kit/core'
import {
  arrayMove,
  sortableKeyboardCoordinates,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable'
import type { CellConfig, NotebookConfig, HorizontalGroupCellConfig } from './notebook-types'

interface UseNotebookDragDropParams {
  cells: CellConfig[]
  notebookConfig: NotebookConfig
  onConfigChange: (config: NotebookConfig) => void
  selectedCellIndex: number | null
  setSelectedCellIndex: (index: number | null) => void
  setSelectedChildName: (name: string | null) => void
}

interface UseNotebookDragDropResult {
  sensors: ReturnType<typeof useSensors>
  hgAwareSortingStrategy: typeof verticalListSortingStrategy
  handleDragStart: (event: DragStartEvent) => void
  handleDragOver: (event: DragOverEvent) => void
  handleDragEnd: (event: DragEndEvent) => void
  activeDragId: string | null
  dragOverZone: 'before' | 'into' | 'after' | null
  dragOverHgName: string | null
}

/**
 * Manages drag-and-drop behavior for notebook cells.
 *
 * Owns:
 * - dnd-kit sensor configuration
 * - Active drag state and drop-zone visual feedback
 * - HG-aware sorting strategy that suppresses transforms during hg hover
 * - Drop zone computation (before/into/after for hg cells)
 * - Reorder and nest-into-hg logic on drag end
 */
export function useNotebookDragDrop({
  cells,
  notebookConfig,
  onConfigChange,
  selectedCellIndex,
  setSelectedCellIndex,
  setSelectedChildName,
}: UseNotebookDragDropParams): UseNotebookDragDropResult {
  const [activeDragId, setActiveDragId] = useState<string | null>(null)
  // Drag zone state for visual feedback (state) + synchronous read in handleDragEnd (refs)
  const [dragOverZone, setDragOverZone] = useState<'before' | 'into' | 'after' | null>(null)
  const [dragOverHgName, setDragOverHgName] = useState<string | null>(null)
  const dragOverZoneRef = useRef<'before' | 'into' | 'after' | null>(null)
  const dragOverHgNameRef = useRef<string | null>(null)

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates })
  )

  // Custom sorting strategy: suppress item transforms when dragging over any
  // hg cell. Without this, the sortable list shifts items when the pointer
  // enters the before/after edge zones, which moves the hg cell and causes
  // the pointer to oscillate between edges — never reaching the "into" zone.
  const hgAwareSortingStrategy = useMemo<typeof verticalListSortingStrategy>(
    () => (args) => {
      if (dragOverHgNameRef.current !== null) {
        return null
      }
      return verticalListSortingStrategy(args)
    },
    []
  )

  const handleDragStart = useCallback((event: DragStartEvent) => {
    setActiveDragId(event.active.id as string)
  }, [])

  // Compute hg drop zone from a drag event's pointer position relative to the over element.
  // Returns the zone ('before'/'into'/'after') and hg name, or null if not over an hg cell.
  const computeHgZone = useCallback(
    (event: { activatorEvent: Event; delta: { x: number; y: number }; active: { id: string | number }; over: { id: string | number; rect: { top: number; height: number } } | null }) => {
      const { active, over } = event
      if (!over) return null

      const overCell = cells.find((c) => c.name === over.id)
      if (!overCell || overCell.type !== 'hg') return null

      // Don't allow dropping an hg cell into another hg
      const activeCell = cells.find((c) => c.name === active.id)
      if (activeCell?.type === 'hg') return null

      const overRect = over.rect
      if (!overRect || !overRect.height) return null

      const pointerY = (event.activatorEvent as PointerEvent)?.clientY
      if (pointerY === undefined) return null

      const currentY = pointerY + event.delta.y
      const relativeY = currentY - overRect.top
      const height = overRect.height

      // Wide "into" zone (80% of height) — the sortable list shifts items during
      // drag, so the pointer naturally lands near edges. Before/after reordering
      // is still possible by targeting the cells adjacent to the hg group.
      let zone: 'before' | 'into' | 'after'
      if (relativeY < height * 0.1) {
        zone = 'before'
      } else if (relativeY > height * 0.9) {
        zone = 'after'
      } else {
        zone = 'into'
      }

      return { zone, hgName: over.id as string }
    },
    [cells]
  )

  const handleDragOver = useCallback(
    (event: DragOverEvent) => {
      const result = computeHgZone(event)
      dragOverZoneRef.current = result?.zone ?? null
      dragOverHgNameRef.current = result?.hgName ?? null
      setDragOverZone(result?.zone ?? null)
      setDragOverHgName(result?.hgName ?? null)
    },
    [computeHgZone]
  )

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      // Compute zone directly from event data (no state/ref timing issues)
      const result = computeHgZone(event)
      setActiveDragId(null)
      setDragOverZone(null)
      setDragOverHgName(null)
      dragOverZoneRef.current = null
      dragOverHgNameRef.current = null

      const { active, over } = event
      if (!over || active.id === over.id) return

      const oldIndex = cells.findIndex((c) => c.name === active.id)
      const overIndex = cells.findIndex((c) => c.name === over.id)
      if (oldIndex === -1 || overIndex === -1) return

      // Check if dropping into an hg cell's middle zone
      const overCell = cells[overIndex]
      const activeCell = cells[oldIndex]
      if (
        overCell.type === 'hg' &&
        result?.zone === 'into' &&
        result?.hgName === over.id &&
        activeCell.type !== 'hg'
      ) {
        // Remove dragged cell from top-level and append to hg's children
        const hgCell = overCell as HorizontalGroupCellConfig
        const newCells = cells.filter((_, i) => i !== oldIndex)
        const hgIndex = newCells.findIndex((c) => c.name === hgCell.name)
        if (hgIndex !== -1) {
          newCells[hgIndex] = {
            ...hgCell,
            children: [...hgCell.children, activeCell],
          }
        }
        onConfigChange({ ...notebookConfig, cells: newCells })
        // Clear selection if the moved cell was selected
        if (selectedCellIndex === oldIndex) {
          setSelectedCellIndex(null)
          setSelectedChildName(null)
        } else if (selectedCellIndex !== null && selectedCellIndex > oldIndex) {
          setSelectedCellIndex(selectedCellIndex - 1)
        }
        return
      }

      // Standard reorder
      const newCells = arrayMove(cells, oldIndex, overIndex)
      const newConfig = { ...notebookConfig, cells: newCells }
      onConfigChange(newConfig)

      // Update selected cell index if needed
      if (selectedCellIndex === oldIndex) {
        setSelectedCellIndex(overIndex)
      } else if (selectedCellIndex !== null) {
        if (oldIndex < selectedCellIndex && overIndex >= selectedCellIndex) {
          setSelectedCellIndex(selectedCellIndex - 1)
        } else if (oldIndex > selectedCellIndex && overIndex <= selectedCellIndex) {
          setSelectedCellIndex(selectedCellIndex + 1)
        }
      }
    },
    [cells, notebookConfig, onConfigChange, selectedCellIndex, computeHgZone, setSelectedCellIndex, setSelectedChildName]
  )

  return {
    sensors,
    hgAwareSortingStrategy,
    handleDragStart,
    handleDragOver,
    handleDragEnd,
    activeDragId,
    dragOverZone,
    dragOverHgName,
  }
}
